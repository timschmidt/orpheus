# Add a driver

Drivers live in `fw/atj2157-hal` (the SoC) and `fw/yoton-bsp` (this
board). The specification per block is [03](../../plan/03-hal-drivers.md);
hardware facts are in [01](../../plan/01-hardware.md). This is device work:
read the [safety rules](../getting-started.md#8-working-with-the-player-safely)
first.

1. **Registers.** Add the block to `fw/atj2157-pac`, hand-written, one
   module per block, every field tagged with its source (datasheet
   section, or the reference code it was confirmed against). Do not copy
   code from the Rockbox tree (GPL); it is a reference for *what* the
   hardware does, not text to reuse.
2. **The driver.** A module in `atj2157-hal`: blocking setup, async
   operations on Embassy primitives, **DMA for every data path with a
   deadline** (audio, card, flash, display, UART). Anything an interrupt
   handler calls must be resident: add it to `resident.toml` and run
   `cargo xtask check-resident`. If it must run while the flash is busy,
   mark it `#[inline(never)]` too.
3. **Board specifics** (pins, ladders, which instance) go in `yoton-bsp`.
4. **Logic on the host.** Whatever can be separated from register access
   (protocol state machines, parsers, calculations) goes into a `crates/`
   crate with host tests, like `orpheus-sdcard` or `orpheus-bt`.
5. **A bring-up image.** `fw/bringup/<name>`: a small RAM-only binary
   (under 96 KiB) that exercises the driver and reports through the result
   block:

   ```sh
   cargo xtask dev ram <name> --test <test>   # loads it, waits, collects results
   ```

   It must end in a reboot, never with the player off or waiting for a
   key.
6. **Unknowns and facts.** What you learn about the hardware goes into
   01 §9 and the source document, **in the same commit** as the code that
   relies on it. Sequences copied from the vendor without understanding are
   marked `VERBATIM` in a comment.
7. **Limits.** PMU regulator and charger fields only take values the stock
   firmware uses (01 §7), charge current never above the 200 mA code; the
   eFuse block is read-only.

Interrupts are masked with BASEPRI through the HAL's critical section;
never PRIMASK (`cortex_m::interrupt::free`, `cpsid i`): a page fault under
PRIMASK is fatal.
