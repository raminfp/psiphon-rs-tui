# psiphon-tui

یک TUI به زبان **Rust** (با [ratatui](https://ratatui.rs)) برای موتور [psiphon-tunnel-core](https://github.com/Psiphon-Labs/psiphon-tunnel-core) که همان ورودی خط‌فرمان مورد نظر را می‌پذیرد:

```
psiphon -config psiphon.config -serverList server-list-standard.txt -dataRootDirectory data
```

## معماری

```
┌────────────────────┐   FFI (cgo)   ┌────────────────────────────────┐
│  Rust TUI (ratatui) │◄─────────────►│ libpsiphon_bridge.so (Go)      │
│  src/*.rs           │  poll notices │ psiphon-core/RustBridge        │
└────────────────────┘               │   ↓ uses                       │
                                      │ github.com/Psiphon-Labs/       │
                                      │ psiphon-tunnel-core/psiphon    │
                                      │ (vendored, unmodified)         │
                                      └────────────────────────────────┘
```

- **`psiphon-core/`** — کد اصلی Psiphon، به‌صورت کامل و بدون تغییر، از commit
  [`a70e0b58`](https://github.com/Psiphon-Labs/psiphon-tunnel-core/commit/a70e0b58c68377dcdfd7b081c0054bf9c2aae1c8)
  گرفته شده (نگاه کنید به `VENDOR_COMMIT`).
- **`psiphon-core/RustBridge/bridge.go`** — تنها فایل جدیدی که به سورس اضافه شده:
  یک شیم cgo که با `go build -buildmode=c-shared` به `libpsiphon_bridge.so` تبدیل می‌شود. برخلاف
  `ClientLibrary` استاندارد (که Blocking است و هیچ notice ای را بیرون نمی‌دهد)، این bridge:
  - تونل را async استارت می‌کند (`PsiphonStart` سریع برمی‌گردد، نه بعد از اتصال کامل)،
  - تمام notice های Psiphon (همان JSON eventهایی که کلاینت‌های Android/iOS/ConsoleClient تولید می‌کنند)
    را در یک صف قرار می‌دهد که Rust با `PsiphonPollNotice` آن را می‌خواند — یعنی TUI وضعیت اتصال را
    زنده (live) نشان می‌دهد، نه فقط یک نتیجهٔ نهایی.
- **`src/`** — کراسیت Rust:
  - `ffi.rs` — بایندینگ خام `extern "C"`
  - `psiphon.rs` — wrapper امن + ترد poller پس‌زمینه
  - `notice.rs` — پارس/نمایش انسانی notice های JSON
  - `app.rs` — state machine وضعیت اتصال
  - `cli.rs` — پارسر آرگومان‌ها (`-config` / `-serverList` / `-dataRootDirectory`، دقیقاً مثل ConsoleClient خود Psiphon)
  - `ui.rs` / `main.rs` — رندر ratatui و event loop
- **`build.rs`** — قبل از بیلد Rust، خودش `go build` را روی bridge اجرا می‌کند، `.so` را کنار باینری نهایی
  کپی می‌کند و rpath می‌زند تا نیازی به `LD_LIBRARY_PATH` نباشد.

## پیش‌نیازها

- Go ≥ 1.26 (توسط `go.mod` upstream الزامی شده؛ اگر نصب نیست، toolchain به‌صورت خودکار دانلود می‌شود –
  به بخش «شبکه‌های محدود» زیر نگاه کنید)
- Rust/Cargo (edition 2021)
- روی لینوکس تست و تأیید شده (خروجی: `libpsiphon_bridge.so`)

### شبکه‌های محدود (GOPROXY)

اگر `proxy.golang.org` / `dl.google.com` در دسترس نبود (که در sandbox این پروژه هم همینطور بود)، از یک
mirror استفاده کنید:

```bash
export GOPROXY=https://goproxy.cn,direct
export GOSUMDB=sum.golang.org
```

`build.rs` همین مقدار را به‌صورت پیش‌فرض ست می‌کند، مگر اینکه شما خودتان `GOPROXY` را در محیط تعریف کرده باشید.

## بیلد و اجرا

```bash
cargo build --release
./target/release/psiphon \
  -config psiphon.config \
  -serverList server-list-standard.txt \
  -dataRootDirectory data
```

(`cargo run --release -- -config ... -serverList ... -dataRootDirectory ...` هم کار می‌کند.)

اولین بار که `cargo build` را اجرا کنید، `build.rs` خودش bridge را کامپایل می‌کند (حدود ۲۰-۳۰ ثانیه، عمدتاً
دانلود toolchain Go در صورت نیاز). بیلدهای بعدی سریع‌اند مگر اینکه `bridge.go` تغییر کرده باشد.

## ⚠️ نکتهٔ مهم: فایل کانفیگ و سرورلیست واقعی

این پروژه **موتور واقعی** Psiphon را اجرا می‌کند، اما نمی‌تواند به‌جای شما مقادیر محرمانه‌ای مثل
`PropagationChannelId`، `SponsorId` یا فهرست سرورهای واقعی (`TargetServerEntry`) را تولید کند — این‌ها
توسط Psiphon Inc. به هر دیپلویمنت اختصاص داده می‌شوند و در سورس عمومی موجود نیستند.

دو گزینه دارید:

1. **از psiphon.config واقعی خودتان استفاده کنید** (مثلاً از یک بیلد رسمی Android/Windows Psiphon که
   دسترسی به آن دارید) و مسیرش را با `-config` بدهید.
2. **یک سرور تست شخصی راه بیندازید** (کاملاً محلی، برای توسعه/تست):

   ```bash
   cd psiphon-core/Server
   go build -o psiphond .
   ./psiphond -ipaddress 127.0.0.1 -protocol OSSH:9999 generate
   # server-entry.dat و *.config تولید می‌شوند؛ محتوای server-entry.dat را
   # در فیلد TargetServerEntry فایل client config قرار دهید (یا با -serverList بدهید)
   ./psiphond run &
   ```

   جزئیات کامل در `psiphon-core/README.md` بخش «Generate configuration data».

فایل‌های `psiphon.config.example` و `server-list-standard.txt.example` در ریشهٔ پروژه فقط نمونه‌ی ساختار
هستند (با placeholder `FFFFFFFFFFFFFFFF`) — بدون مقادیر واقعی، برنامه بالا می‌آید، notice های زنده را
نشان می‌دهد، اما به هیچ سروری وصل نمی‌شود (این رفتار تست شده و مورد انتظار است).

## کلیدهای TUI

| کلید | عملکرد |
|---|---|
| `s` | شروع/اتصال مجدد (اگر idle/stopped/error باشد) |
| `x` | قطع اتصال بدون خروج از برنامه |
| `r` | باز کردن پنل انتخاب کشور (region) |
| `q` / `Esc` | خروج (با graceful shutdown) |
| `↑`/`k`، `↓`/`j` | اسکرول لاگ (یا حرکت در لیست کشورها وقتی پنل `r` باز است) |
| `PgUp`/`PgDn` | اسکرول سریع‌تر |
| `End` | پرش به انتهای لاگ (live) |
| `Enter` (در پنل `r`) | اعمال کشور انتخابی و ریست خودکار اتصال |
| `Esc` (در پنل `r`) | بستن پنل بدون تغییر |

هنگام اجرا، برنامه بلافاصله (مثل CLI اصلی Psiphon) سعی می‌کند وصل شود؛ نیازی به فشردن `s` در ابتدا نیست.

### انتخاب کشور (Region)

با کلید `r` پنلی باز می‌شود که فقط کشورهایی را نشان می‌دهد که خودِ Psiphon از طریق notice واقعی
`AvailableEgressRegions` گزارش کرده — یعنی صرفاً کشورهایی که واقعاً در بین server entry هایی که کلاینت
شما (از `-serverList` یا اتصال‌های قبلی) دارد وجود دارند؛ لیست از پیش‌تعیین‌شده یا ساختگی نیست. تا وقتی
حداقل یک بار موفق به دریافت entry از یک/چند کشور نشده باشید، فقط گزینهٔ «Any» دیده می‌شود.

با انتخاب یک کشور و زدن `Enter`:
- اگر تونل روشن بود، اول به‌صورت خودکار قطع (`Stop`) و بعد از تأیید کامل خاموش‌شدن، با فیلتر `EgressRegion`
  جدید دوباره وصل می‌شود (`state: Stopping → Stopped → Starting`، بدون نیاز به فشردن `s`).
- اگر خاموش بود، بلافاصله با همان فیلتر تلاش برای اتصال شروع می‌شود.

کد کشورها (`US`, `DE`, ...) با نام کامل کشور (`United States`, `Germany`, ...) در `src/regions.rs` نمایش داده
می‌شوند؛ این فایل فقط برای نمایش زیباست، منبع لیست انتخاب‌شدنی نیست.

## تست بدون رابط گرافیکی

یک ابزار تشخیصی headless هم هست که همان مسیر FFI را امتحان می‌کند و خروجی notice ها را چاپ می‌کند
(مفید برای دیباگ یک config واقعی قبل از اجرای کامل TUI):

```bash
cargo run --example smoke -- psiphon.config server-list-standard.txt data 15
```

## پنل‌های TUI چه چیزی نشان می‌دهند

- **Proxy**: پورت‌های SOCKS/HTTP لوکال (از notice های `ListeningSocksProxyPort`/`ListeningHttpProxyPort`)
  و تعداد تونل‌های فعال (`Tunnels`)
- **Session**: ناحیهٔ کلاینت/سرور، حجم ترافیک (`TotalBytesTransferred`)، تعداد homepage های اسپانسر
- **Log**: استریم زندهٔ همهٔ notice ها با رنگ‌بندی بر اساس severity (خطا/هشدار/اطلاعات)

## بهبود مقاومت در برابر DPI (انتخاب پروتکل)

اگر پروتکل پیش‌فرض (اغلب OSSH خام) در شبکه‌تان شناسایی/مسدود می‌شود، اول ببینید سرورهای واقعی‌تان اصلاً چه
پروتکل‌هایی را پشتیبانی می‌کنند — حدس نزنید. `server-list-standard.txt` هر خطش یک server entry هگزادسیمال
است؛ با پکیج خودِ Psiphon (`psiphon-core/psiphon/common/protocol`, تابع `NewStreamingServerEntryDecoder` +
`(*ServerEntry).SupportsProtocol`) می‌شود شمارش گرفت که چند سرور از هرکدام از این پروتکل‌ها پشتیبانی
می‌کنند: `OSSH`، `TLS-OSSH`، `UNFRONTED-MEEK-HTTPS-OSSH`، `UNFRONTED-MEEK-SESSION-TICKET-OSSH`،
`FRONTED-MEEK-*` (fronting از طریق CDN مثل Cloudflare — ممکن است صفر سرور آن را داشته باشد)، و
`INPROXY-WEBRTC-OSSH` (جدیدترین و سخت‌ترین برای شناسایی — شبیه ترافیک تماس تصویری واقعی).

نکات مهمی که در عمل پیدا شد:

- کلیدهایی که در JSON کانفیگ ناشناخته باشند **بی‌صدا نادیده گرفته می‌شوند** (`psiphon.LoadConfig` از
  `json.Unmarshal` ساده استفاده می‌کند، نه decoder سخت‌گیر) — یعنی یک تایپوی کوچک در اسم فیلد هیچ خطایی
  نمی‌دهد و فقط بی‌اثر می‌ماند. قبل از اعتماد به یک فیلد کانفیگ، مطمئن شوید همان اسم دقیق در
  `psiphon-core/psiphon/config.go` وجود دارد.
- `INPROXY-WEBRTC-OSSH` (پروتکل WebRTC) نیاز به «broker specs» دارد که فقط از طریق **Tactics** (سیستم
  تنظیم از راه دور خودِ Psiphon) به کلاینت می‌رسد؛ اگر `"DisableTactics": true` باشد، این پروتکل عملاً
  همیشه رد می‌شود ولو این‌که سرورها پشتیبانی‌اش کنند (نگاه کنید به پیام
  `"inproxy client: no broker specs and tactics disabled"` در `psiphon-core/psiphon/controller.go`).
- برای اولویت‌دادن به پروتکل‌های مخفی‌تر بدون قفل‌شدن کامل روی آن‌ها (اگر هیچ‌کدام وصل نشدند)، از
  `InitialLimitTunnelProtocols` + `InitialLimitTunnelProtocolsCandidateCount` استفاده کنید — فقط
  N کاندیدای اول را به این پروتکل‌ها محدود می‌کند، بعدش `LimitTunnelProtocols` (که خالی/نامحدود می‌ماند)
  اعمال می‌شود:

  ```json
  {
    "InitialLimitTunnelProtocols": [
      "INPROXY-WEBRTC-OSSH",
      "UNFRONTED-MEEK-SESSION-TICKET-OSSH",
      "UNFRONTED-MEEK-HTTPS-OSSH",
      "TLS-OSSH"
    ],
    "InitialLimitTunnelProtocolsCandidateCount": 50
  }
  ```

  این نمونه واقعاً روی یک لیست ۳۹۱-سروره تست شد: اتصال طی ~۲ ثانیه با `ActiveTunnel` روی `TLS-OSSH` برقرار
  شد (به‌جای OSSH خام).

## مجوز

کد vendor‌شده در `psiphon-core` تحت GPLv3 (نگاه کنید به همان مسیر) از Psiphon Inc.
است. کد Rust/bridge این پروژه هم به همین ترتیب باید GPLv3 در نظر گرفته شود چون به‌طور مستقیم به آن لینک
می‌شود.
