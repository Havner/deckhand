//! Windows-only build step: embed a multi-size application icon into the `.exe` (shown in the
//! taskbar, title bar, and Explorer).
//!
//! The icon is derived at build time from `assets/deckhand.png` - the same 512x512 art the Linux
//! install ships - so that PNG stays the single source of truth. We box-downsample it to the standard
//! Windows icon sizes, assemble them into a `.ico`, and hand that to `winresource` to compile + link.
//! A no-op on non-Windows hosts (where the icon deps aren't even pulled in - see `Cargo.toml`).

fn main() {
    println!("cargo:rerun-if-changed=assets/deckhand.png");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    embed_icon();
}

/// Decode `assets/deckhand.png`, downsample to the common shell icon sizes, write an `.ico` into
/// `OUT_DIR`, and embed it as the application icon.
#[cfg(windows)]
fn embed_icon() {
    use std::path::PathBuf;

    let (rgba, w, h) = decode_png(include_bytes!("assets/deckhand.png"));

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    // ICO entries top out at 256^2; the source (512^2) box-downsamples to each of these by a clean
    // integer factor, so every entry is a crisp average rather than a resampler's guess.
    for size in [256u32, 128, 64, 32, 16] {
        if w % size != 0 || h % size != 0 {
            continue; // non-square / non-divisible source: skip rather than smear it
        }
        let scaled = box_downsample(&rgba, w as usize, h as usize, (w / size) as usize);
        let image = ico::IconImage::from_rgba_data(size, size, scaled);
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("encode ico entry"));
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("deckhand.ico");
    dir.write(std::fs::File::create(&out).expect("create ico"))
        .expect("write ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("ico path is utf-8"));
    res.compile().expect("compile windows resource");
}

/// Decode a PNG to straight RGBA8 with its dimensions.
#[cfg(windows)]
fn decode_png(bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().expect("read png info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png buffer size")];
    let info = reader.next_frame(&mut buf).expect("decode png");
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "icon png must be 8-bit");
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => {
            let mut v = Vec::with_capacity((info.width * info.height * 4) as usize);
            for c in px.as_chunks::<3>().0 {
                v.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            v
        }
        other => panic!("unsupported icon png color type: {other:?}"),
    };
    (rgba, info.width, info.height)
}

/// Average-downsample straight-RGBA `src` by an integer `factor` in both dimensions.
#[cfg(windows)]
fn box_downsample(src: &[u8], sw: usize, sh: usize, factor: usize) -> Vec<u8> {
    if factor <= 1 {
        return src.to_vec();
    }
    let (dw, dh) = (sw / factor, sh / factor);
    let n = (factor * factor) as u32;
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0u32; 4];
            for yy in 0..factor {
                let row = (y * factor + yy) * sw;
                for xx in 0..factor {
                    let si = (row + x * factor + xx) * 4;
                    for (a, s) in acc.iter_mut().zip(&src[si..si + 4]) {
                        *a += *s as u32;
                    }
                }
            }
            let di = (y * dw + x) * 4;
            for (o, a) in out[di..di + 4].iter_mut().zip(acc) {
                *o = (a / n) as u8;
            }
        }
    }
    out
}
