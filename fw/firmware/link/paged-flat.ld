/*
 * Pass 1 of a paged link (the script cargo links with). The ELF cargo
 * produces from it is NOT a runnable image: run `cargo xtask build` (or
 * `cargo xtask pagelink`), which links again with the generated pass-2
 * script that replaces `-Tpaged-flat.ld` in the saved link
 * (tools/orpheus-ld).
 */
INCLUDE memory-flat.ld
INCLUDE layout.ld
