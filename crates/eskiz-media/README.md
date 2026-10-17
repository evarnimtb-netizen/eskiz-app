# eskiz-media

Animation and video export for Eskiz: animated GIF, APNG, H.264 in MP4, and timelapse recording.

Every exporter takes frames as premultiplied RGBA8 (`Frame`) with a per-frame duration. Files are written
through a temporary file and renamed into place.

## API

| Export | Functions | Notes |
|---|---|---|
| GIF | `encode_gif`, `write_gif_file`, `GifOptions` | NeuQuant palettes (`color_quant`), exact palettes when the image has few colors, global or per-frame palette, Floyd-Steinberg dithering, 1-bit transparency with alpha threshold, loop count. Delay is the duration rounded to centiseconds, minimum 2. |
| APNG | `encode_apng`, `write_apng_file`, `ApngOptions` | Full-canvas 8-bit RGBA, straight alpha, partial alpha kept. Delay is the duration in milliseconds over 1000. |
| MP4 | `encode_mp4`, `write_mp4_file`, `Mp4Options` | H.264 High profile via OpenH264, BT.709 tags, bitrate from `bitrate_bps`. Odd sizes are padded to even with the background color. |
| Timelapse | `TimelapseRecorder`, `TimelapseSettings`, `SegmentStore` | Commit frames, downsample into the resolution box, roll segments, export full length or a 30 s summary. |

Spec helpers (`bitrate_bps`, `capture_size`, `frames_per_segment`, `summary_source_indices`, `Quality`,
`Resolution`) are public and pure.

## H.264 backends (Cargo features)

- **None (default):** no OpenH264 and no MP4 muxer (`openh264`, `mp4`) are linked. GIF, APNG and the raw timelapse
  work; MP4 paths return `MP4_UNAVAILABLE`.
- **`source`:** OpenH264 is compiled from source. The `openh264` crate and OpenH264 are BSD-2-Clause.
- **`libloading`:** OpenH264 is loaded at runtime from the path in the `ESKIZ_OPENH264_LIB` environment variable.
  The crate uses the `openh264` crate's `libloading` feature for this.

Build with `--features libloading` to ship without compiling OpenH264.

### Cisco's prebuilt binary

Cisco publishes a prebuilt `libopenh264` binary. Cisco's patent license for H.264 covers that binary, as
long as it is downloaded at runtime from Cisco and used unmodified. A copy compiled from source does not
carry that license (see the licensing notes in the README).

For distributed builds:

1. Build with `--no-default-features --features libloading`, so OpenH264 is not compiled into the app.
2. Have the app download Cisco's binary at runtime from Cisco's site, and set `ESKIZ_OPENH264_LIB` to its path.
3. If the binary is missing, MP4 export reports `MediaError::EncoderUnavailable`. Show that to the user.

Check the terms of Cisco's binary and the H.264 licensing position with counsel before shipping. This note is
not legal advice.

## Dependencies

| Crate | Version | License | Why |
|---|---|---|---|
| `gif` | 0.14 | MIT OR Apache-2.0 | GIF encoder and the test decoder |
| `color_quant` | 1.1 | MIT | NeuQuant palettes (the version `gif` uses) |
| `png` | 0.18 | MIT OR Apache-2.0 | APNG encoder and the test decoder |
| `openh264` | 0.9 | BSD-2-Clause | H.264 encoder and decoder |
| `mp4` | 0.14 | MIT | Pure-Rust MP4 muxer and reader (the tests parse with it too) |
| `thiserror` | 2 | MIT OR Apache-2.0 | Error type |

`muxide` 0.2.5 was looked at as an alternative. It was not evaluated in depth, so `mp4` was chosen. The
upstream activity of `mp4` (last release 0.14.0) was not checked.

## Tests

```
cargo test -p eskiz-media
```

- `tests/gif_apng.rs`: three-frame GIF and APNG round trips, checking frame count, delays, sizes, transparency,
  and partial alpha.
- `tests/mp4.rs`: ten-frame moving square, checking the `ftyp` box, one video track of the right size and
  sample count, and duration. Also checks odd sizes padded to even. Both print a skip message when OpenH264
  cannot initialize.
- `tests/timelapse.rs`: capture sizing, bitrate and segment arithmetic, the summary frame plan, segment roll
  counts, and a full export and a 30 s summary end to end.

## Gaps

- **Timelapse dissolves:** no 0.5 s dissolve between segments. Full-length export copies samples as they are.
- **Mixed-size recordings:** a full export whose segments differ in size is refused. Re-encoding is not
  implemented.
- **Multiple-of-16 coded size:** the H.264 coded size is only padded to even. The usual 16-pixel alignment
  (section 3.6) is not applied.
- **Alpha timelapse (`.mov`, H.265 with alpha):** not implemented. The MP4 output has no alpha.
- **HEVC:** not implemented.
- **Background queue:** frames are encoded on the calling thread. There is no drop queue and no background
  encoder.
- **Memory:** a GIF export holds all prepared frames in memory. The H.264 sample table for one MP4 export is
  also in memory until it is written.
- **Crash recovery and disk-full handling:** not implemented. Segments are not recovered after a crash.
- **Summary duration:** the summary plan follows the standard plan for N of 72 or more. For shorter recordings, Eskiz
  holds the warp at source frame 50 (an Eskiz choice).
