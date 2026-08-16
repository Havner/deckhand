# Introduction

This document gathers some general notes related to the engine and the
profiles. Inner workings, advanced use cases, edge cases or anything that is
worth writing down but goes beyond regular documentation or manuals.

# Activation/Gaters

Most `SourceBindings` (things you can map the `SourceInput` into, things like
Joystick mouse / As mouse / Directional pad, etc) contain `Activation`
setting. This setting defines when the operation should be active or not. It has
two modes: `Hold to disable`/`Hold to enable` and a `Gaters` list.

`Hold to disable` means the operation is always active unless one of the gaters
is pressed/held (no gaters -> always enabled).

`Hold to enable` means the operation is always inactive unless one of the gaters
is pressed/held (no gaters -> always disabled).

This option makes the most sense for two operations:

- Gyro to mouse: where it serves a simple purpose of when the gyro should be
  enabled, on what button press
- Directional pad: it has the same implementation on both, joystick and the
  touchpad. For a Directional pad on a touchpad to make sense, you usually want
  to press the touchpad (contrary to joystick where deflection is enough). The
  gater here is used to distinguish the two. The default Directional pad configuration
  on touchpads contains that touchpad's click as a gater, so usually nothing
  more needs to be done. If you removed the gater it would be enough to simply
  touch the pad to trigger the direction (which is equivalent to joystick's
  deflection).

The `Gaters` can only be real physical buttons that are reported by the hardware
bits. You can see them on the gaters/buttons widget shown when adding a new
gater. No virtual buttons can be used here, as at gater-eval time the virtual
buttons have not been evaluated yet. That includes the Trigger Soft press. In
theory it could be added as a pseudo physical button, but only with a hardcoded
threshold and I wanted the threshold to be fully configurable.

If you need to gate something (e.g. Gyro) behind a virtual button (e.g. Trigger
Soft press, Directional pad direction or Outer-ring) you can do that using
layers. Create a layer that has e.g. Gyro always enabled (`Hold to disable` + no
gater) and activate that layer as `Hold` on the virtual button. This will
work. Which brings me to the next topic:

# Layers/ActionSets edge cases

There are 4 commands to change layers/action-sets:
`HoldLayer`/`AddLayer`/`RemoveLayer`/`ChangeActionSet`.

3 are persistent/immediate, one is active on hold. They all change the effective
bindings of buttons for the mapper (either layers or action sets). Let's call a
currently active one an OperationalSet (a combination of an Action set and its
layers on top, OpSet in short here), doesn't really matter whether it's a Layer
or Action set for the purpose of this discussion.

As stated in previous section a virtual button can be used to perform an OpSet
change. And it can, but there is an edge case here. Imagine a situation where
you press a button that changes an OpSet and that OpSet rebinds the button that
was used to change the OpSet.

The following describes only such a situation: **where a button used to change the
OpSet is being rebound on the new OpSet**. All other cases work fine as is.

If the button is a physical button (e.g. A, X, Trigger full press, Shoulder
button) it will behave consistently. The OpSet will get changed and on a next
press of the button (assuming it was persistent change) the button will have a
new function.

However if the button used to change the OpSet in such a case (that the new
OpSet rebinds it) is a virtual button this will not behave properly. The OpSet
might flicker (be added/removed each tick). This is the edge case. I don't think
anyone would map AddLayer to a joystick's Directional pad direction, but this
cannot be fully defined, as the OpSet resolver cannot dedup/latch on a virtual
button properly to handle the edge case. The important thing to add is that such
an ill-defined operation will never produce a stuck key or some other undefined
state when the button is released.

Trigger soft pull is kind of a special case here. It is a virtual button in
principle, but as long as the original mapping and the new mapping in the new
OpSet are configured to the same soft press threshold this will behave
properly. If the new OpSet changes the threshold to a point our soft press is
actually not pressed in the new OpSet the flicker of layers/action-sets might
occur as well.

# Latching/Deduplication

For this edge case (changing to an OpSet with a button that is rebound in the
new OpSet) to be handled properly such buttons need to be deduplicated or
latched for the operation not to flicker. And such deduplication or latching is
only possible for buttons that can be evaluated from a raw controller frame
(that's why virtual buttons do not behave properly).

And for such deduplication the OpSet change needs to be performed only on the
rising-edge of the action. All the others that happen afterwards need to be
deduplicated. This has a side effect of not being able to trigger OpSet changes
with one button press if they happen in different moments in time (more than one
happening at the same tick is fine). It's not about not being able to map two
OpSet changes on one button press. Just not being able to trigger them on
different ticks.

E.g. such a mapping:

- Regular(interruptible): AddLayer(layer); Long(300): ChangeActionSet(set)
- Long(300): AddLayer(layer1); Long(300): AddLayer(layer2)

Will work properly, cause such bindings can only produce one tick that triggers
an OpSet change. However bindings like those:

- Regular(non-interruptible): AddLayer(layer); Long(300): ChangeActionSet(set)
- Long(300): AddLayer(layer1); Long(500): AddLayer(layer2)

can trigger OpSet changes on one button press at different moments in time. Such
bindings will only ever trigger the OpSet changes in the first such tick. All
the others will get masked. Not sure anyone would ever want to
bind things this way (I definitely wouldn't), but documenting the edge cases
anyway.
