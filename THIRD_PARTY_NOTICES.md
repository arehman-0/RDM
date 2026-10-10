# Third-party notices

IDM Replica uses the software below. This file is **generated** — do not edit it by hand;
run `node scripts/gen-third-party-notices.mjs` after changing dependencies or `components.json`.

## 1. Tools downloaded at first run

These are separate programs. IDM Replica downloads each one from the pinned URL in
`src-tauri/components.json`, verifies its SHA-256, and runs it as its own process.

### yt-dlp 2026.08.19

- License: **GPL-3.0-or-later (the bundled exe); yt-dlp's own code: Unlicense**
- Homepage: https://github.com/yt-dlp/yt-dlp
- Pinned download: https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe
- SHA-256: `66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a` (17.0 MB)
- Downloaded unmodified from the official GitHub release and run as a separate program; IDM Replica does not link against it or include its code. yt-dlp's own code is released under the Unlicense, but its README states that the PyInstaller-bundled executable includes GPLv3+ licensed code, so that exe as a whole is licensed under GPLv3+ (https://www.gnu.org/licenses/gpl-3.0.txt); see THIRD_PARTY_LICENSES.txt in the yt-dlp repository. Source: https://github.com/yt-dlp/yt-dlp at the release tag.

### QuickJS-NG 0.17.0

- License: **MIT**
- Homepage: https://github.com/quickjs-ng/quickjs
- Pinned download: https://github.com/quickjs-ng/quickjs/releases/download/v0.17.0/qjs-windows-x86_64.exe
- SHA-256: `2aeabf0092c3262d6b2609824418f7dd7ed1f1df939f73b2b15645230cac0d77` (2.1 MB)
- Downloaded unmodified from the official QuickJS-NG GitHub release and run as a separate program. Source: https://github.com/quickjs-ng/quickjs.

### FFmpeg n9.0.2-17-g2a571b6068 (LGPL, shared)

- License: **LGPL-3.0-or-later**
- Homepage: https://github.com/BtbN/FFmpeg-Builds
- Pinned download: https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-30-13-08/ffmpeg-n9.0.2-17-g2a571b6068-win64-lgpl-shared-9.0.zip
- SHA-256: `7157177b8a6cb2174c1650ba8c71b363f2c78cba5330f88c4c02cf5b2b880646` (73.4 MB)
- Downloaded unmodified from BtbN/FFmpeg-Builds (an LGPL build: no GPL or non-free components) and run as a separate program; IDM Replica does not link against it. FFmpeg is licensed under the GNU Lesser General Public License version 3 or later; the full text is in licenses/FFMPEG-LGPL-3.0.txt. Source code: https://github.com/FFmpeg/FFmpeg at the commit named in the version (g2a571b6068), and the exact build scripts and patches at https://github.com/BtbN/FFmpeg-Builds (release autobuild-2026-09-30-13-08).

## 2. Rust crates linked into the app (263)

License summary:

- MIT OR Apache-2.0: 119
- MIT: 59
- Apache-2.0 OR MIT: 19
- Unicode-3.0: 18
- MIT/Apache-2.0: 14
- Unlicense OR MIT: 6
- MPL-2.0: 5
- BSD-3-Clause: 3
- Apache-2.0: 3
- Unlicense/MIT: 2
- 0BSD OR MIT OR Apache-2.0: 1
- BSD-3-Clause AND MIT: 1
- BSD-3-Clause/MIT: 1
- Zlib OR Apache-2.0 OR MIT: 1
- CC0-1.0 OR MIT-0 OR Apache-2.0: 1
- (Apache-2.0 OR MIT) AND BSD-3-Clause: 1
- Apache-2.0 / MIT: 1
- MIT / Apache-2.0: 1
- Apache-2.0/MIT: 1
- MIT AND BSD-3-Clause: 1
- MIT OR Zlib OR Apache-2.0: 1
- MIT OR Apache-2.0 OR Zlib: 1
- Apache-2.0 OR BSL-1.0: 1
- (MIT OR Apache-2.0) AND Unicode-3.0: 1
- BSD-2-Clause OR Apache-2.0 OR MIT: 1

Most are MIT and/or Apache-2.0 (the standard texts: https://opensource.org/license/mit and
https://www.apache.org/licenses/LICENSE-2.0); each crate's own copyright notice and license file
is in its source repository, linked below. MPL-2.0 crates are used unmodified; their source is at
the linked repository.

| Package | Version | License | Source |
|---|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://github.com/oyvindln/adler2 |
| aho-corasick | 1.1.4 | Unlicense OR MIT | https://github.com/BurntSushi/aho-corasick (*) |
| alloc-no-stdlib | 2.0.4 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib |
| alloc-stdlib | 0.2.2 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib (*) |
| anyhow | 1.0.102 | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| async-trait | 0.1.89 | MIT OR Apache-2.0 | https://github.com/dtolnay/async-trait (*) |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/atomic-waker |
| axum | 0.7.9 | MIT | https://github.com/tokio-rs/axum |
| axum-core | 0.4.5 | MIT | https://github.com/tokio-rs/axum |
| base64 | 0.21.7 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| base64 | 0.22.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| bitflags | 1.3.2 | MIT/Apache-2.0 | https://github.com/bitflags/bitflags |
| bitflags | 2.12.0 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| brotli | 7.0.0 | BSD-3-Clause AND MIT | https://github.com/dropbox/rust-brotli |
| brotli-decompressor | 4.0.3 | BSD-3-Clause/MIT | https://github.com/dropbox/rust-brotli-decompressor |
| bstr | 1.12.1 | MIT OR Apache-2.0 | https://github.com/BurntSushi/bstr |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 | https://github.com/fitzgen/bumpalo |
| bytemuck | 1.25.0 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| byteorder | 1.5.0 | Unlicense OR MIT | https://github.com/BurntSushi/byteorder |
| bytes | 1.11.1 | MIT | https://github.com/tokio-rs/bytes |
| cfb | 0.7.3 | MIT | https://github.com/mdsteele/rust-cfb |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| color_quant | 1.1.0 | MIT | https://github.com/image-rs/color_quant.git |
| convert_case | 0.4.0 | MIT | https://github.com/rutrum/convert-case |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| crc32fast | 1.5.0 | MIT OR Apache-2.0 | https://github.com/srijs/rust-crc32fast (*) |
| crossbeam-channel | 0.5.15 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-deque | 0.8.6 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-epoch | 0.9.18 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-utils | 0.8.21 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| cssparser | 0.27.2 | MPL-2.0 | https://github.com/servo/rust-cssparser (*) |
| cssparser-macros | 0.6.1 | MPL-2.0 | https://github.com/servo/rust-cssparser (*) |
| ctor | 0.2.9 | Apache-2.0 OR MIT | https://github.com/mmastrac/rust-ctor (*) |
| darling | 0.23.0 | MIT | https://github.com/TedDriggs/darling |
| darling_core | 0.23.0 | MIT | https://github.com/TedDriggs/darling (*) |
| darling_macro | 0.23.0 | MIT | https://github.com/TedDriggs/darling |
| derive_more | 0.99.20 | MIT | https://github.com/JelteF/derive_more (*) |
| digest | 0.10.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| dirs-next | 2.0.0 | MIT OR Apache-2.0 | https://github.com/xdg-rs/dirs |
| dirs-sys-next | 0.1.2 | MIT OR Apache-2.0 | https://github.com/xdg-rs/dirs/tree/master/dirs-sys |
| displaydoc | 0.2.6 | MIT OR Apache-2.0 | https://github.com/yaahc/displaydoc (*) |
| dtoa | 1.0.11 | MIT OR Apache-2.0 | https://github.com/dtolnay/dtoa |
| dtoa-short | 0.3.5 | MPL-2.0 | https://github.com/upsuper/dtoa-short |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | https://gitlab.com/kornelski/dunce |
| encoding_rs | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause | https://github.com/hsivonen/encoding_rs |
| equivalent | 1.0.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/equivalent |
| fastrand | 2.4.1 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | https://github.com/image-rs/fdeflate |
| filetime | 0.2.29 | MIT/Apache-2.0 | https://github.com/alexcrichton/filetime |
| flate2 | 1.1.9 | MIT OR Apache-2.0 | https://github.com/rust-lang/flate2-rs (*) |
| fnv | 1.0.7 | Apache-2.0 / MIT | https://github.com/servo/rust-fnv |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| futf | 0.1.5 | MIT / Apache-2.0 | https://github.com/servo/futf |
| futures-channel | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs (*) |
| futures-core | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-macro | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-sink | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-task | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-util | 0.3.32 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs (*) |
| fxhash | 0.2.1 | Apache-2.0/MIT | https://github.com/cbreeden/fxhash |
| generic-array | 0.14.7 | MIT | https://github.com/fizyk20/generic-array.git (*) |
| getrandom | 0.1.16 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom (*) |
| getrandom | 0.2.17 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| getrandom | 0.4.2 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom (*) |
| glob | 0.3.3 | MIT OR Apache-2.0 | https://github.com/rust-lang/glob |
| globset | 0.4.18 | Unlicense OR MIT | https://github.com/BurntSushi/ripgrep/tree/master/crates/globset |
| hashbrown | 0.12.3 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| heck | 0.5.0 | MIT OR Apache-2.0 | https://github.com/withoutboats/heck |
| html5ever | 0.26.0 | MIT OR Apache-2.0 | https://github.com/servo/html5ever (*) |
| http | 0.2.12 | MIT OR Apache-2.0 | https://github.com/hyperium/http (*) |
| http | 1.4.1 | MIT OR Apache-2.0 | https://github.com/hyperium/http (*) |
| http-body | 1.0.1 | MIT | https://github.com/hyperium/http-body (*) |
| http-body-util | 0.1.3 | MIT | https://github.com/hyperium/http-body (*) |
| http-range | 0.1.5 | MIT | https://github.com/bancek/rust-http-range.git |
| httparse | 1.10.1 | MIT OR Apache-2.0 | https://github.com/seanmonstar/httparse |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | https://github.com/pyfisch/httpdate |
| hyper | 1.10.0 | MIT | https://github.com/hyperium/hyper (*) |
| hyper-tls | 0.6.0 | MIT/Apache-2.0 | https://github.com/hyperium/hyper-tls |
| hyper-util | 0.1.20 | MIT | https://github.com/hyperium/hyper-util (*) |
| ico | 0.4.0 | MIT | https://github.com/mdsteele/rust-ico |
| icu_collections | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| icu_locale_core | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| icu_normalizer | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_normalizer_data | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_properties | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_properties_data | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_provider | 2.2.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| ident_case | 1.0.1 | MIT/Apache-2.0 | https://github.com/TedDriggs/ident_case |
| idna | 1.1.0 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | https://github.com/hsivonen/idna_adapter |
| ignore | 0.4.25 | Unlicense OR MIT | https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore |
| image | 0.24.9 | MIT OR Apache-2.0 | https://github.com/image-rs/image |
| indexmap | 1.9.3 | Apache-2.0 OR MIT | https://github.com/bluss/indexmap |
| indexmap | 2.14.0 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/indexmap (*) |
| infer | 0.13.0 | MIT | https://github.com/bojand/infer |
| instant | 0.1.13 | BSD-3-Clause | https://github.com/sebcrozet/instant |
| ipnet | 2.12.0 | MIT OR Apache-2.0 | https://github.com/krisprice/ipnet |
| itoa | 0.4.8 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| itoa | 1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| json-patch | 2.0.0 | MIT/Apache-2.0 | https://github.com/idubrov/json-patch |
| jsonptr | 0.4.7 | MIT OR Apache-2.0 | https://github.com/chanced/jsonptr |
| kuchikiki | 0.8.2 | MIT | https://github.com/brave/kuchikiki |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 | https://github.com/rust-lang-nursery/lazy-static.rs |
| libc | 0.2.186 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| litemap | 0.8.2 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| log | 0.4.30 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| mac | 0.1.1 | MIT/Apache-2.0 | https://github.com/reem/rust-mac.git |
| markup5ever | 0.11.0 | MIT OR Apache-2.0 | https://github.com/servo/html5ever |
| matches | 0.1.10 | MIT | https://github.com/SimonSapin/rust-std-candidates |
| matchit | 0.7.3 | MIT AND BSD-3-Clause | https://github.com/ibraheemdev/matchit |
| md-5 | 0.10.6 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| memchr | 2.8.1 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| mime | 0.3.17 | MIT OR Apache-2.0 | https://github.com/hyperium/mime |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide (*) |
| mio | 1.2.1 | MIT | https://github.com/tokio-rs/mio |
| native-tls | 0.2.18 | MIT OR Apache-2.0 | https://github.com/rust-native-tls/rust-native-tls (*) |
| new_debug_unreachable | 1.0.6 | MIT | https://github.com/mbrubeck/rust-debug-unreachable |
| nodrop | 0.1.14 | MIT/Apache-2.0 | https://github.com/bluss/arrayvec |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | https://github.com/rust-num/num-traits |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot (*) |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| phf | 0.10.1 | MIT | https://github.com/sfackler/rust-phf |
| phf | 0.11.3 | MIT | https://github.com/rust-phf/rust-phf |
| phf | 0.8.0 | MIT | https://github.com/sfackler/rust-phf (*) |
| phf_generator | 0.11.3 | MIT | https://github.com/rust-phf/rust-phf |
| phf_generator | 0.8.0 | MIT | https://github.com/sfackler/rust-phf |
| phf_macros | 0.11.3 | MIT | https://github.com/rust-phf/rust-phf (*) |
| phf_macros | 0.8.0 | MIT | https://github.com/sfackler/rust-phf (*) |
| phf_shared | 0.10.0 | MIT | https://github.com/sfackler/rust-phf |
| phf_shared | 0.11.3 | MIT | https://github.com/rust-phf/rust-phf (*) |
| phf_shared | 0.8.0 | MIT | https://github.com/sfackler/rust-phf |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| png | 0.17.16 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png (*) |
| potential_utf | 0.1.5 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 | https://github.com/cryptocorrosion/cryptocorrosion (*) |
| precomputed-hash | 0.1.1 | MIT | https://github.com/emilio/precomputed-hash |
| proc-macro-hack | 0.5.20+deprecated | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro-hack |
| proc-macro2 | 1.0.106 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 (*) |
| quote | 1.0.45 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote (*) |
| rand | 0.7.3 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand | 0.8.6 | MIT OR Apache-2.0 | https://github.com/rust-random/rand (*) |
| rand_chacha | 0.2.2 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_core | 0.5.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand (*) |
| rand_core | 0.6.4 | MIT OR Apache-2.0 | https://github.com/rust-random/rand (*) |
| rand_pcg | 0.2.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| raw-window-handle | 0.5.2 | MIT OR Apache-2.0 OR Zlib | https://github.com/rust-windowing/raw-window-handle |
| regex-automata | 0.4.14 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex (*) |
| regex-syntax | 0.8.10 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| reqwest | 0.12.28 | MIT OR Apache-2.0 | https://github.com/seanmonstar/reqwest |
| rfd | 0.10.0 | MIT | https://github.com/PolyMeilex/rfd |
| rustls-pki-types | 1.14.1 | MIT OR Apache-2.0 | https://github.com/rustls/pki-types |
| rustversion | 1.0.22 | MIT OR Apache-2.0 | https://github.com/dtolnay/rustversion |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | https://github.com/dtolnay/ryu |
| same-file | 1.0.6 | Unlicense/MIT | https://github.com/BurntSushi/same-file |
| schannel | 0.1.29 | MIT | https://github.com/steffengy/schannel-rs |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | https://github.com/bluss/scopeguard |
| selectors | 0.22.0 | MPL-2.0 | https://github.com/servo/servo |
| semver | 1.0.28 | MIT OR Apache-2.0 | https://github.com/dtolnay/semver (*) |
| serde | 1.0.228 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde (*) |
| serde_core | 1.0.228 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_derive | 1.0.228 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde (*) |
| serde_json | 1.0.150 | MIT OR Apache-2.0 | https://github.com/serde-rs/json (*) |
| serde_repr | 0.1.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/serde-repr |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 | https://github.com/nox/serde_urlencoded |
| serde_with | 3.20.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ |
| serde_with_macros | 3.20.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ (*) |
| serialize-to-javascript | 0.1.2 | MIT OR Apache-2.0 | https://github.com/chippers/serialize-to-javascript |
| serialize-to-javascript-impl | 0.1.2 | MIT OR Apache-2.0 | https://github.com/chippers/serialize-to-javascript |
| servo_arc | 0.1.1 | MIT/Apache-2.0 | https://github.com/servo/servo |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| sharded-slab | 0.1.7 | MIT | https://github.com/hawkw/sharded-slab |
| simd-adler32 | 0.3.9 | MIT | https://github.com/mcountryman/simd-adler32 |
| siphasher | 0.3.11 | MIT/Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| siphasher | 1.0.3 | MIT/Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| slab | 0.4.12 | MIT | https://github.com/tokio-rs/slab |
| smallvec | 1.15.1 | MIT OR Apache-2.0 | https://github.com/servo/rust-smallvec |
| socket2 | 0.6.4 | MIT OR Apache-2.0 | https://github.com/rust-lang/socket2 (*) |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | https://github.com/storyyeller/stable_deref_trait |
| state | 0.5.3 | MIT/Apache-2.0 | https://github.com/SergioBenitez/state |
| string_cache | 0.8.9 | MIT OR Apache-2.0 | https://github.com/servo/string-cache |
| strsim | 0.11.1 | MIT | https://github.com/rapidfuzz/strsim-rs |
| syn | 1.0.109 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn (*) |
| syn | 2.0.117 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn (*) |
| sync_wrapper | 1.0.2 | Apache-2.0 | https://github.com/Actyx/sync_wrapper (*) |
| synstructure | 0.13.2 | MIT | https://github.com/mystor/synstructure (*) |
| tao | 0.16.11 | Apache-2.0 | https://github.com/tauri-apps/tao |
| tar | 0.4.46 | MIT OR Apache-2.0 | https://github.com/composefs/tar-rs |
| tauri | 1.8.3 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri (*) |
| tauri-codegen | 1.4.6 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri/tree/dev/core/tauri-codegen |
| tauri-macros | 1.4.7 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| tauri-plugin-single-instance | 0.0.0 | Apache-2.0 OR MIT | — |
| tauri-runtime | 0.14.6 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri (*) |
| tauri-runtime-wry | 0.14.11 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| tauri-utils | 1.6.2 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri (*) |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | https://github.com/Stebalien/tempfile |
| tendril | 0.4.3 | MIT/Apache-2.0 | https://github.com/servo/tendril |
| thin-slice | 0.1.1 | MPL-2.0 | https://github.com/heycam/thin-slice |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror (*) |
| thiserror | 2.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror (*) |
| thiserror-impl | 2.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thread_local | 1.1.9 | MIT OR Apache-2.0 | https://github.com/Amanieu/thread_local-rs |
| tinystr | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| tokio | 1.52.3 | MIT | https://github.com/tokio-rs/tokio (*) |
| tokio-macros | 2.7.0 | MIT | https://github.com/tokio-rs/tokio |
| tokio-native-tls | 0.3.1 | MIT | https://github.com/tokio-rs/tls (*) |
| tokio-util | 0.7.18 | MIT | https://github.com/tokio-rs/tokio |
| tower | 0.5.3 | MIT | https://github.com/tower-rs/tower (*) |
| tower-http | 0.6.11 | MIT | https://github.com/tower-rs/tower-http |
| tower-layer | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| tower-service | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| tracing | 0.1.44 | MIT | https://github.com/tokio-rs/tracing (*) |
| tracing-attributes | 0.1.31 | MIT | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | https://github.com/tokio-rs/tracing (*) |
| tracing-subscriber | 0.3.23 | MIT | https://github.com/tokio-rs/tracing |
| try-lock | 0.2.5 | MIT | https://github.com/seanmonstar/try-lock |
| typenum | 1.20.0 | MIT OR Apache-2.0 | https://github.com/paholg/typenum |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| unicode-segmentation | 1.13.2 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-segmentation |
| url | 2.5.8 | MIT OR Apache-2.0 | https://github.com/servo/rust-url (*) |
| utf-8 | 0.7.6 | MIT OR Apache-2.0 | https://github.com/SimonSapin/rust-utf8 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT | https://github.com/hsivonen/utf8_iter |
| uuid | 1.23.2 | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid (*) |
| walkdir | 2.5.0 | Unlicense/MIT | https://github.com/BurntSushi/walkdir (*) |
| want | 0.3.1 | MIT | https://github.com/seanmonstar/want |
| webview2-com | 0.19.1 | MIT | https://github.com/wravery/webview2-rs (*) |
| webview2-com-macros | 0.6.0 | MIT | https://github.com/wravery/webview2-rs |
| webview2-com-sys | 0.19.0 | MIT | https://github.com/wravery/webview2-rs |
| winapi | 0.3.9 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| winapi-util | 0.1.11 | Unlicense OR MIT | https://github.com/BurntSushi/winapi-util (*) |
| windows | 0.37.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows | 0.39.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs (*) |
| windows_x86_64_msvc | 0.37.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_msvc | 0.39.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-implement | 0.39.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs (*) |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs (*) |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-tokens | 0.39.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-version | 0.1.7 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| writeable | 0.6.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| wry | 0.24.12 | Apache-2.0 OR MIT | https://github.com/tauri-apps/wry |
| yoke | 0.8.2 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| yoke-derive | 0.8.2 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zerocopy | 0.8.49 | BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/google/zerocopy |
| zerofrom | 0.1.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zerofrom-derive | 0.1.7 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zeroize | 1.8.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| zerotrie | 0.2.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zerovec | 0.11.6 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zerovec-derive | 0.11.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x (*) |
| zip | 2.4.2 | MIT | https://github.com/zip-rs/zip2.git |
| zmij | 1.0.21 | MIT | https://github.com/dtolnay/zmij |
| zopfli | 0.8.3 | Apache-2.0 | https://github.com/zopfli-rs/zopfli |

## 3. npm packages bundled into the UI (6)

| Package | Version | License | Source |
|---|---|---|---|
| @tauri-apps/api | 1.6.0 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| js-tokens | 4.0.0 | MIT | lydell/js-tokens |
| loose-envify | 1.4.0 | MIT | git://github.com/zertosh/loose-envify |
| react | 18.3.1 | MIT | https://github.com/facebook/react |
| react-dom | 18.3.1 | MIT | https://github.com/facebook/react |
| scheduler | 0.23.2 | MIT | https://github.com/facebook/react |

The browser extension (`/extension`) has no runtime dependencies.
