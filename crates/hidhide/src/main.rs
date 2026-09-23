// hidhide - run a command with matching HID devices hidden from it (Linux).
//
// The Linux analog of Windows HidHide, scoped to a single process tree: create an
// unprivileged user + mount namespace, bind-mount /dev/null over every device node
// whose HID vendor:product matches, then exec the command. The command and its
// children can no longer open the real device (open() resolves to /dev/null, whose
// ioctls fail, so hidapi/evdev enumerate it but skip it); every other process on the
// system is untouched, and the masking vanishes when the command exits.
//
// The bind mounts are set up once, at launch, so a device that appears AFTERWARDS is
// not hidden. A plugged Steam Controller dongle already exposes its phantom slots
// whether or not a pad is powered on, so powering a pad on later still lands on a
// masked slot; a wired/BT controller plugged in after launch would not be hidden.

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("hidhide: Linux-only (needs user + mount namespaces)");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use clap::Parser;

    /// Hide HID devices matching VENDOR:PRODUCT from a command, then run the command.
    ///
    /// Ids are hex. With no -p, every device of the vendor is hidden.
    /// Example: hidhide -- steam   (hides all Valve devices from Steam)
    #[derive(Parser)]
    #[command(version, about, long_about = None)]
    struct Args {
        /// Vendor id, hex (default: Valve = 28DE).
        #[arg(short = 'v', long, value_parser = parse_hex, default_value = "28DE")]
        vendor: u16,
        /// Product id, hex; repeat -p to hide several. Omit to hide every device of
        /// this vendor.
        #[arg(short = 'p', long = "product", value_parser = parse_hex)]
        products: Vec<u16>,
        /// Mask only /dev/hidraw*, leaving the kernel evdev/js gamepad nodes visible.
        #[arg(long)]
        hidraw_only: bool,
        /// Command to run, with its arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    }

    fn parse_hex(s: &str) -> Result<u16, String> {
        let t = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
        u16::from_str_radix(t, 16).map_err(|e| format!("invalid hex id '{s}': {e}"))
    }

    pub fn main() {
        let args = Args::parse();
        if let Err(e) = run(&args) {
            eprintln!("hidhide: {e}");
            std::process::exit(1);
        }
    }

    fn run(args: &Args) -> Result<(), String> {
        let nodes = matching_nodes(args.vendor, &args.products, args.hidraw_only);
        if nodes.is_empty() {
            let pids = if args.products.is_empty() {
                "*".to_string()
            } else {
                args.products.iter().map(|p| format!("{p:04x}")).collect::<Vec<_>>().join(",")
            };
            eprintln!(
                "hidhide: no device matches {:04x}:{pids}; running with nothing hidden",
                args.vendor
            );
        } else {
            hide(&nodes)?;
        }
        // exec replaces this process image (hidhide leaves memory); returns on failure only.
        let err = Command::new(&args.command[0]).args(&args.command[1..]).exec();
        Err(format!("exec '{}': {err}", args.command[0]))
    }

    /// /dev nodes whose HID vendor:product matches.
    fn matching_nodes(vendor: u16, products: &[u16], hidraw_only: bool) -> Vec<PathBuf> {
        let mut nodes = Vec::new();
        // hidraw: device/uevent carries HID_ID=<bus>:<vendor>:<product>, all hex. This
        // covers USB and BLE (BLE has no id/vendor in the sysfs chain, but HID_ID is set).
        if let Ok(rd) = std::fs::read_dir("/sys/class/hidraw") {
            for e in rd.flatten() {
                let Ok(txt) = std::fs::read_to_string(e.path().join("device/uevent")) else {
                    continue;
                };
                let Some(id) = txt.lines().find_map(|l| l.strip_prefix("HID_ID=")) else {
                    continue;
                };
                let mut f = id.split(':').skip(1); // skip bus
                let v = f.next().and_then(|x| u32::from_str_radix(x, 16).ok());
                let p = f.next().and_then(|x| u32::from_str_radix(x, 16).ok());
                if let (Some(v), Some(p)) = (v, p) {
                    if v as u16 == vendor && want(products, p as u16) {
                        nodes.push(Path::new("/dev").join(e.file_name()));
                    }
                }
            }
        }
        // evdev/js: the kernel gamepad the hid-steam driver also exposes. Hide it too,
        // else Steam simply reads that instead of the hidraw node. id/{vendor,product}
        // are 4-hex.
        if !hidraw_only {
            if let Ok(rd) = std::fs::read_dir("/sys/class/input") {
                for e in rd.flatten() {
                    let name = e.file_name();
                    let n = name.to_string_lossy();
                    if !(n.starts_with("event") || n.starts_with("js")) {
                        continue;
                    }
                    let id = e.path().join("device/id");
                    let v = read_hex(&id.join("vendor"));
                    let p = read_hex(&id.join("product"));
                    if let (Some(v), Some(p)) = (v, p) {
                        if v == vendor && want(products, p) {
                            nodes.push(Path::new("/dev/input").join(&name));
                        }
                    }
                }
            }
        }
        nodes.sort();
        nodes.dedup();
        nodes
    }

    /// An empty product list means "any product of this vendor".
    fn want(products: &[u16], pid: u16) -> bool {
        products.is_empty() || products.contains(&pid)
    }

    fn read_hex(p: &Path) -> Option<u16> {
        u16::from_str_radix(std::fs::read_to_string(p).ok()?.trim(), 16).ok()
    }

    /// New unprivileged user + mount namespace, then bind /dev/null over each node.
    fn hide(nodes: &[PathBuf]) -> Result<(), String> {
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) } != 0 {
            return Err(oserr("unshare(CLONE_NEWUSER|CLONE_NEWNS)"));
        }
        // Identity-map our uid/gid: we stay ourselves, but hold CAP_SYS_ADMIN inside the
        // new userns, which is what makes the bind mounts permitted. setgroups must be
        // denied before gid_map (kernel rule for an unprivileged map).
        wr("/proc/self/setgroups", "deny")?;
        wr("/proc/self/uid_map", &format!("{uid} {uid} 1"))?;
        wr("/proc/self/gid_map", &format!("{gid} {gid} 1"))?;
        // Make mounts private so our binds never propagate back to the host.
        mnt(None, Path::new("/"), libc::MS_REC | libc::MS_PRIVATE)
            .map_err(|e| format!("make-rprivate /: {e}"))?;
        for node in nodes {
            mnt(Some("/dev/null"), node, libc::MS_BIND)
                .map_err(|e| format!("bind /dev/null -> {}: {e}", node.display()))?;
        }
        Ok(())
    }

    fn wr(path: &str, val: &str) -> Result<(), String> {
        std::fs::write(path, val).map_err(|e| format!("write {path}: {e}"))
    }

    fn mnt(src: Option<&str>, tgt: &Path, flags: libc::c_ulong) -> Result<(), std::io::Error> {
        let src_c = src.map(|s| CString::new(s).unwrap());
        let tgt_c = CString::new(tgt.as_os_str().as_bytes()).unwrap();
        let src_ptr = src_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        let ret = unsafe {
            libc::mount(src_ptr, tgt_c.as_ptr(), std::ptr::null(), flags, std::ptr::null())
        };
        if ret != 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }

    fn oserr(ctx: &str) -> String {
        format!("{ctx}: {}", std::io::Error::last_os_error())
    }
}
