# Security

## Reporting a vulnerability

Please report security problems **privately** through GitHub's form:
<https://github.com/timschmidt/orpheus/security/advisories/new>. Only the
maintainers can read it. Do not open a public issue for a vulnerability.

Say what is affected (firmware, host tools, website), how to reproduce it,
and what an attacker gains. Orpheus is maintained by a small team: an
answer can take a while. Fixes are made on `main`;
there are no maintained release branches yet.

What counts: anything that lets a file on the card, a USB host, a Bluetooth
peer or a web page run code on the player, damage the flash or the
battery, read data it should not, or that makes the host tools write where
they should not. Bugs in the parsers (audio, pictures, tags, file system)
are welcome too; they are fuzzed, and every finding stays in the test
corpus.

## What the player can and cannot protect

The YM03 has **no secure storage**. Its boot ROM lets anyone with the
player and a USB cable read the whole flash chip and the chip's eFuse
words. That is not something firmware can change.

### The authenticator (planned)

The authenticator app (one-time codes, [06 §5](docs/plan/06-connectivity.md))
is designed and its code library (`orpheus-otp`) exists, but the app is
**not built yet**. When it is, its limits are:

* Secrets are encrypted in the flash vault (XChaCha20-Poly1305) with a key
  derived from your PIN or passphrase (PBKDF2-HMAC-SHA-256, about one
  second of work per guess on the player, mixed with the chip's id).
* **A 4–8 digit PIN only stops casual access through the menus.** Someone
  who has the player and a cable can copy the flash and try every PIN
  offline. Only a **long passphrase** protects the secrets against that.
  The set-up screen says so.
* There is no secure element. The feature must never be described as
  hardware-backed.
* Wrong-PIN delays (1 s doubling to 5 min) only slow down guessing on the
  player itself.
* Decrypted secrets are in RAM only while the app is unlocked and are
  erased on lock, screen-off and before a reboot into recovery mode.

### The developer channel

Every USB personality of the firmware answers a small vendor-specific
protocol on endpoint 0 (logs, screenshots, key injection, test commands,
memory reads). It exists so the project can be tested without hands on
the keys. Anyone who can plug the player into a computer can use it; it is
not a security boundary. The same is true of the chip's recovery mode.

### The card

Files on the card are not encrypted. The firmware writes only inside
`/.orpheus/` on the card.

## Supported versions

Only the current `main` branch.
