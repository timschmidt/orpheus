# Fixed bugs and their regression tests

Owner's policy (2026-11-25, 07 §3.1): every fixed bug has a host test
`regress_<bug>` that fails without the fix, or an on-device check where only
the player shows it. `cargo test regress_` runs the host set; fuzz and soak
inputs replay from `tests/regress/` on every `cargo test`. Sources: STATUS,
CHANGELOG, commit log up to `acc73592`; backfilled 2026-11-25.

Status: **covered** (a test existed; renamed `regress_…` or given a
`Regression:` line), **new** (written in the backfill), **device** (only
the player shows it; the check is named), **infra** (a test-tool fix, no
test needed). **R**: checked by reverting the fix locally (the test failed,
then passed with it).

## Product bugs

| Bug | Date | Fix | Test or check | Status |
|---|---|---|---|---|
| USB-W1: SETUP read not drop-safe, EP0 deaf until a restart | 11-24 | `c7b46d7c` | `orpheus-usb/tests/ep0_model.rs::regress_usb_w1_setup_read_is_drop_safe`; `dev usb-cancel` | covered (the model's `Before` flavour is the unfixed driver) |
| USB-W1: an overtaken transfer's status landed on the next one (PEEK stall) | 11-24 | `c7b46d7c` | `ep0_model.rs::regress_usb_w1_overtaken_transfer_is_given_up` | covered (`Before`) |
| USB-W1: a bus reset did not re-arm EP0 | 11-24 | `c7b46d7c` | `ep0_model.rs::regress_usb_w1_bus_reset_re_arms_ep0` | covered |
| USB-W1: nothing recovered a deaf controller | 11-24 | `c7b46d7c` | `orpheus_usb::liveness::regress_usb_w1_liveness_recovers_the_incident` | covered |
| USB-W1: bulk OUT pending at a reset waited for ever; VBUS loss did not re-initialise | 11-24 | `c7b46d7c` | `dev usb-cancel --rounds 3`, user check 28 (re-plug) | device (HAL driver state) |
| Stale test request survived warm reboots (soak quit to USB recovery) | 11-25 | `4d5af5d7` | `orpheus_proto::mailbox::regress_stale_test_request` | covered |
| Music › Shuffle all reset the player (watchdog) on a large library | 11-25 | `b7949dc9` | `orpheus-library/tests/library.rs::regress_shuffle_all_sample_in_bounded_steps` | covered |
| Fault reboot cleared the boot counter (no slot fallback on a crash) | 11-24 | `699e2cf4` | `orpheus_proto::mailbox::regress_fault_reboot_keeps_the_boot_counter` (decision moved to `mailbox::fault_reboot`) | new, R |
| Radio RAM overwritten by the step counter's (SRAM3 overlap) | 11-24 | `ebc145a5` | `orpheus_proto::mem::regress_sram3_blocks_chained_not_overlapping`; firmware `const` asserts per block (`xtask build`) | new (block sizes are firmware types: the build asserts them) |
| Tuner id read 0000 (no fallback to 00h at 0x11) | 11-24 | `ebc145a5` | `orpheus_fm::chip::regress_fm_identify_falls_back_to_register_00_at_0x11` | covered |
| `fm on` with music playing left the radio off | 11-24 | `ebc145a5` | `fm on` during `playdir` (STATUS lead check) | device (firmware task glue) |
| Recorder captured silence (mic front end on the wrong side) | 11-24 | `596ee066` | `atj2157_pac::adc::regress_mic_front_end_is_the_left_side` (values moved to `adc::mic_mix_for_mic`/`ctl_for_mic`) | new, R |
| Analogue output near-silent: PA volume 0 at power-up | 11-24 | `94e9e62e` | `dev cmd "fm status"`: `pa_ctl` bits 14:12 = 7; user check 9 | device (VERBATIM register sequence in the HAL) |
| Bluetooth volume keys without effect on a non-scaling sink | 11-24 | `84085395` | `orpheus_bt::proto::regress_bt_volume_keys_act_at_the_dac` (policy moved to `proto::dac_level`, `SINK_VOLUME_SENT`) | new, R |
| Bluetooth stream silent: 0x26 never sent | 11-24 | `ba89d1ce` | `orpheus_bt::transmitter::connect_sends_player_volume` | covered |
| Bluetooth addresses shown byte-reversed | 11-24 | `ba89d1ce` | `orpheus_bt::proto::events` | covered |
| F-BT-1: sink volume above 127 set the player to maximum | 11-24 | `5c6dd83e` | `tests/regress/bt_tx/`, `tests/regress/bt_frame/`; `proto::volume_scaling` | covered (corpus) |
| F-SET-1: settings programmed over bytes after the log end | 11-24 | `5c6dd83e` | `orpheus_settings::regress_settings_garbage_after_the_log_end`; `tests/regress/settings_mount/program-over-garbage-after-log-end` | covered |
| F-USB-1: READ DVD STRUCTURE head over the allocation length | 11-24 | `5c6dd83e` | `tests/regress/usb_scsi/` | covered (corpus) |
| F-IDX-1: letter jump past the listing on a damaged index | 11-24 | `5c6dd83e` | `tests/regress/index_open/` | covered (corpus) |
| F-LIB-1, F-LIB-2: library offset overflow, `file_len` below the header | 11-24 | `5c6dd83e` | `tests/regress/library_open/` | covered (corpus) |
| F-PLY-1, F-PLY-2: old shuffle order on an empty queue; repeat one on an empty queue | 11-24 | `5c6dd83e` | `tests/regress/player_resume/` | covered (corpus) |
| F-UI-2: content or menu redrawn over the volume overlay | 11-24 | `5c6dd83e` | `tests/regress/ui_keys/content-…`, `menu-redrawn-over-volume-overlay` | covered (corpus) |
| F-UI-3: remaining time a second off | 11-24 | `5c6dd83e` | `tests/regress/ui_keys/remaining-time-off-by-one` | covered (corpus) |
| Screen under an open menu redrawn with a moved marquee | 11-24 | `9d9cdfa6` | `tests/regress/ui_keys/menu-underlay-marquee-moved` (corpus input `58133401…`, kept) | covered (corpus) |
| Stopwatch: the Prev release paged the alarm list | 11-24 | `9d9cdfa6` | `sim/tests/tools.rs::alarm_list_edits` | covered |
| Rolling round hid a settings group's title | 11-24 | `9d9cdfa6` | `orpheus_ui::widgets::list::rolling_to_the_top_shows_the_header_above` | covered |
| Store-key collision: radio frequency on the step goal's 0x0D20 | 11-24 | `97e60988` | `orpheus_apps::settings::table_is_consistent`; `id_tests::regress_service_kinds_screen_ids_setting_ids_unique` | covered + new |
| Service-kind collision AUTH = RECORDER = 6 (and M7 screen ids) | 11-24 | `b9c2f00e` | `orpheus_apps::id_tests::regress_service_kinds_screen_ids_setting_ids_unique` (reads the sources: every `other::`, `data::`, screen id, `settings::id`) | new, R |
| Recorder and USB Drive missing from Home (`app_available`) | 11-24 | `43903ea8` | `orpheus_apps::tests::regress_every_app_is_on_the_players_home` (list moved to `App::on_player`); suite `home` | new, R |
| About shortened "Hardware" (long hardware string) | 11-24 | `437c2d1c` | `orpheus_apps::tests::regress_about_hardware_row_fits` (string shared: `services::PLAYER_HARDWARE`) | new, R |
| Card info always "No card" | 11-24 | `437c2d1c` | suite `system`, shot `card` | device (card-task publishing) |
| UI task stack too deep (2,352 B free) | 11-24 | `6efb4131` | suite `stack free` line; soak/fuzz-ui stack oracle (`[hil] stack_free_min_bytes`) | device (static frame sizes) |
| `playdir` during playback ignored | 11-24 | `1872ded9` | `orpheus_proto::devchan::regress_playdir_during_playback_stops_the_track` (`devchan::stops_track_first`) | new, R |
| Bit rate shown as 0 kbit/s | 11-24 | `1872ded9` | `orpheus_apps::services::bitrate_shown` | covered |
| Browsing Files while playing: 8–14 underruns per track | 11-24 | `1872ded9` | soak `browse-while-playing` (`ub: serve` oracle) | device (timing) |
| Card not remounted after `usb msc ro` / `usb dev` | 11-24 | `1872ded9` | soak card hand-over; `usb msc ro` then `usb dev`, `status` | device |
| `playdir` rebooted the player (paging near thrash after a layout shift) | 11-24 | `9f4898fd` | soak `music` (pager loads/s oracle) | device (layout, `hot.toml`) |
| Battery logger never wrote while idle | 11-24 | `9f4898fd` | `orpheus_power::drain::log_header_name_and_rows` | covered |
| `clock 288` stuck at Max under Normal | 11-24 | `f34851c6` | `orpheus_power::governor::regress_max_found_under_normal_leaves_at_once` | covered |
| Opus: "stream needs a larger ring" (pages above the ring) | 11-24 | `cceac998` | `orpheus-ogg` `tests::assembled::regress_ogg_pages_larger_than_the_ring` | covered |
| Opus: "decoder too slow" (paging) | 11-24 | `730427ff` | `playdir` Opus, `status` `dec:` loads | device (performance) |
| `ub:` underrun causes did not add up | 11-24 | `ec43e388` | `orpheus_audio::ledger::every_underrun_is_charged_once_to_its_cause` | covered |
| Gapless joins underran (lookups, preparation, hand-over) | 11-24 | `879d9674`, `ec43e388` | `playdir` join statistics in `status` | device (timing) |
| NOR write executed garbage: NOR code paged | 11-24 | `8e4a6243` | `xtask check-resident` (NOR module a root); shuffle toggles while playing | device + static |
| NOR write crash: `program_page` inlined into paged code | 11-24 | `d1fb600d` | Show hidden toggles; `#[inline(never)]` (check-resident cannot see inlined copies) | device |
| PCM ring deadlock after an underrun (unsigned compare) | 11-24 | `5798fba7` | soak underrun oracle | device (RT path) |
| DSP chain hang at 48 MHz | 11-24 | `dea83c42` | `dsp on` during playback; soak `dsp` actions | device (performance) |
| DAC ran during a seek (7 underruns per seek) | 11-24 | `5dc9abc9` | soak seeks (underruns outside seek windows) | device |
| Served requests counted towards the too-slow abort | 11-24 | `bb6d618b` | browse while playing (no abort) | device |
| Main stack overflow (`Fw` 6 KB by value); `Fw::want` re-requested rows of short folders | 11-24 | `f9e52dbc` | stack oracle; suite `files` | device |
| First NOR boot hung: start-up code paged before the pager | 11-23 | `8494e775` | `xtask check-resident` (start-up a root) | device + static |
| Card hand-back stalled (mode channel and command queue full) | 11-23 | `a76401da` | soak card hand-over | device |
| Garbled marquee in screenshots (frame read while redrawing) | 11-24 | `cf426e32` | suite shots | device |
| Shuffle-all walk starved the UI (1 ms per 16 entries) | 11-24 | `7caf4cf5` | superseded by the library sample (`regress_shuffle_all_…`) | covered |
| Soak navigation to Now Playing opened Shuffle all (roll-around wrap) | 11-25 | `b7949dc9` + this backfill | `xtask` `dev::soak::tests::regress_soak_now_playing_does_not_open_shuffle_all` (simulator rig) | new, R (see below) |
| Underruns inside a half not counted (soak: headroom −13 frames, `underruns=0`) | 11-25 | this commit | `orpheus-audio/tests/regress_pcm_headroom.rs::regress_overrun_inside_half_uncounted`; `tests/regress/soak/mixed-ring-seed1-7.soak` | new |
| Headroom read one half low when the half interrupt fell between its two reads | 11-25 | this commit | `regress_pcm_headroom.rs::regress_headroom_irq_between_reads` | new, R |
| Soak finding hid that a low-water covers the time since the previous sample (an earlier action) | 11-25 | this commit | `xtask` `dev::soak::tests::regress_ring_finding_names_the_span_it_covers` | new |
| AU-PERF: playback's per-block lines evicted each other (77-line `play-block`; hot lines 32 KiB apart share a line of the direct-mapped cache): ~3,000 line loads/s with Pictures open | 11-25 | AU-PERF | `xtask` `pagelink::tests::regress_clusters_that_run_together_share_no_cache_line` (the `fits_with` check `pagelink` runs on every build); `dev prof --faults … --only "browse pictures"` | new (device numbers: lead) |
| AU-PERF: chores of 25–55 ms forced in after 300 ms of waiting at any headroom (soak seed 1, `browse pictures` with a 256-track shuffled MP3 queue: `pcm: head -93 … ub=serve:25ms`, `-265 … 28ms`, `-649 … 55ms`) | 11-25 | AU-PERF (chore budgets) | `orpheus-audio` `chore::tests::regress_forced_chore_never_exceeds_headroom` (host model; `old_rule_underruns_under_the_soak_load` shows the old rule fails it); `dev soak --replay tests/regress/soak/mixed-underrun-seed1-6.soak` | new (device run: lead) |
| AU-PERF: the software I2C's "100 kHz" ran at ≈ 30 kHz (`spin_us` spins ≈ 3× as long), the step counter's samples ≈ 8 % of the CPU | 11-25 | AU-PERF | `steps status` `bus_us_avg` ≈ 1,000 µs with nothing playing (was ≈ 3,000); U-I2C-1 | device (HAL timing) |
| AU-PERF: a request kind learnt its need from a variant that reads no card (`want` while a folder plays, `drain`/`sensor` once the file is chosen): its card-reading variant (`want` in a library list) was then admitted at a budget of a few ms | 11-25 | AU-PERF (`ub=` labels) | `orpheus-audio` `chore::tests::regress_a_piece_without_card_work_does_not_lower_the_need` (host model; the old measurement runs the ring dry on every seed) | new, R |
| AU-PERF: the underrun ledger named only the cause (`ub=serve:55ms`), kept an operation run while the DAC was stopped as the candidate, and a large chore overrun could be hidden by the one-line-a-second limit (soak seed 1, `browse pictures` → `radio-keys`) | 11-25 | AU-PERF (`ub=` labels) | `orpheus-audio` `ledger::tests::regress_the_candidate_keeps_its_detail`; device: the `pcm:` line's `ub=<path>` (lead) | new |
| Soak: underruns read in `radio-keys` over the 28 s since the last sample in `browse pictures` (a read spanned two actions; the radio's entry, which stops the music, had no read before it) | 11-25 | AU-PERF (`ub=` labels) | `xtask` `dev::soak::tests::regress_underruns_are_read_in_the_action_they_happen_in` | new, R |
| PG-1 (resident room): the 17,794 B UI font was one rodata section larger than a 16 KiB rodata group, so pagelink kept it resident behind a warning (17.8 KB of the 80 KiB resident image; 45 B left) | 11-26 | PG-1 (this commit) | `orpheus_ui::font::tests::regress_ui_font_resident_as_one_oversized_section` (every face in pieces ≤ 16 KiB, every glyph as the whole file's); `xtask` `pagelink::tests::regress_oversized_ui_font_was_resident` (an oversized section stops the link) | new |
| TTS-3: the speech lexicon (32,766 B) was one rodata static larger than a 16 KiB rodata group: the speech build stopped in pagelink | 11-27 | TTS-3 (this commit) | `orpheus_tts::lexicon::tests::regress_lexicon_larger_than_a_rodata_group` (three pieces ≤ 16 KiB cut at block starts, every entry and lookup as from the whole file) | new |
| PG-2: the speech voice with the screen reader (133 pages) and Books (139) did not fit the 128-page code window (both: 148 pages, 1,206,358 B), so neither went into the image | 11-27 | PG-2 (this commit) | `xtask` `pagelink::tests::regress_speech_and_books_did_not_fit_the_code_window` (the speech+books build's paged sections, `tests/regress/pagelink/speech-books-2026-11-27.txt`: without cold code the window overflows; with it both windows keep room) | new |
| PG-2b: firmware update from a PG-2 image wrote at 2.5 KB/s and the watchdog reset the player in the read-back (SHA-256's round loop cold, straddling the physical line of its constants); the card path starved the UI into the watchdog | 11-27 | PG-2b | `xtask` `rules::tests::regress_update_data_path_ran_from_the_rodata_window`, `pagelink::tests::cold_code_that_conflicts_with_its_constants_comes_back_first`; `dev update` timing (≈ 40 s) | new, R (device check pending: the fixed image must first be staged) |
| PG-2b: MIDI underran more after PG-2 — the synth's per-piece constants on the wavetable's rodata lines (1,170 rodata faults/s, 2,297 line loads/s in "Etude") | 11-27 | PG-2b | `pagelink::tests::regress_synth_piece_constants_on_the_wavetable_lines`; `playdir …/midi` underruns, `dev stats` loads/s | new, R (device check pending) |
| PG-2b: idle Home ≈ 200 line loads/s (≈ 120 before): `Fw::setting` and `SettingsCache::get` on one physical code line (the 2026-11-25 bug again, by layout) | 11-27 | PG-2b | `rules::tests::regress_setting_reads_evicted_each_other`; the suite's idle-Home check | new, R (device check pending) |
| PG-2b: `status` said `idle … clk=288/288` long after MIDI stopped (the synth's last step, not the clock); `dev stats` printed `clock level ?` at 288 MHz | 11-27 | PG-2b | `orpheus_synth::load::tests::regress_idle_status_claimed_the_synth_clock`, `xtask` `dev::stats_tests::regress_stats_clock_level_max_unnamed` | new |

## Test-infrastructure fixes (no `regress_` test needed)

| Fix | Date | Commit |
|---|---|---|
| `orpheus-img` fixture race (threads shared `gen-<pid>`) | 11-24 | `2edb690c` |
| LSan: `orpheus-apps` strings test leaked its packs; simulator `Media` cache (S-1) | 11-24 | `b200489f`, `5c6dd83e` |
| exfatprogs 1.2 lacks `mkfs -q/-P`, `dump.exfat -s/-d`: cross-checks from 1.4 | 11-24 | `0a84019f`, `afa1c410`, `931a9b5e` |
| ImageMagick 6 `convert` as well as 7 `magick` | 11-24 | `70c87542` |
| Browser test clean-up retries (Chromium still writing its profile) | 11-24 | `70ea66a7` |
| A2DP capture through a null sink (`pw-record` recorded the wrong stream) | 11-24 | `84085395` |
| `usbimage-test`: device nodes retried while udev applies permissions | 11-24 | `d6342f09` |
| Suite: marquee re-reads, full-layout precondition, masks, 30 s fixtures, variants | 11-24 | `6efb4131` |
| Soak simulator rig parsed `settings test-set` ids as hexadecimal (every set refused) | 11-25 | this backfill |
| Soak: list roll-around "off" had no effect under the test look | 11-25 | this backfill |

The last two were found writing the soak test: the test look reads every
look setting, roll-around included, as its default, so `clamp_lists` could
not stop the wrap on the player either; the session now navigates to Now
Playing with the look off. The test fails with either part reverted.

## Still open (not fixed)

F-SET-2 (`remove` on a full store), F-UI-1 (overlay corners), F-UI-4 (Now
Playing redraw), RTC stopped after some resets, AU-0b ticking, the first
FLAC gapless join (3–4 underruns), album-art dropouts, "Start original
firmware" offered in the full layout (`dual_boot()`), no language packs on
the player.
