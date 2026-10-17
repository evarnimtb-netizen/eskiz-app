//! GIF and APNG round trips: encode three frames, decode them with the `gif` and `png` decoders, and check
//! frame count, delays, sizes, transparency and partial alpha.

use std::io::Cursor;
use std::time::Duration;

use eskiz_media::{encode_apng, encode_gif, ApngOptions, Frame, GifOptions, PaletteMode};

const W: u32 = 40;
const H: u32 = 30;

/// Opaque red square at `x`, on a transparent background, premultiplied.
fn square_frame(x: u32, duration: Duration) -> Frame {
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    for y in 8..20 {
        for px in x..x + 10 {
            let i = ((y * W + px) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    Frame::new(W, H, pixels, duration).unwrap()
}

fn three_frames() -> Vec<Frame> {
    let fps15 = Duration::from_secs_f64(1.0 / 15.0);
    vec![
        square_frame(2, fps15),
        square_frame(10, fps15),
        square_frame(18, fps15),
    ]
}

#[test]
fn gif_round_trip_three_frames() {
    let frames = three_frames();
    let mut bytes = Vec::new();
    encode_gif(&mut bytes, &frames, &GifOptions::default()).unwrap();

    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(&bytes)).unwrap();
    assert_eq!((decoder.width(), decoder.height()), (W as u16, H as u16));
    assert!(
        matches!(decoder.repeat(), gif::Repeat::Infinite),
        "default loop count is forever"
    );

    let mut delays = Vec::new();
    while let Some(frame) = decoder.read_next_frame().unwrap() {
        assert_eq!((frame.width, frame.height), (W as u16, H as u16));
        delays.push(frame.delay);
    }
    // 1/15 s is 6.67 centiseconds, which rounds to 7.
    assert_eq!(delays, vec![7, 7, 7]);
}

#[test]
fn gif_per_frame_palette_and_dither_decode() {
    let frames = three_frames();
    let options = GifOptions {
        palette: PaletteMode::PerFrame,
        dither: true,
        ..GifOptions::default()
    };
    let mut bytes = Vec::new();
    encode_gif(&mut bytes, &frames, &options).unwrap();

    let mut decode = gif::DecodeOptions::new();
    decode.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = decode.read_info(Cursor::new(&bytes)).unwrap();
    let mut count = 0;
    while decoder.read_next_frame().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn gif_transparency_keeps_transparent_pixels() {
    // Opaque blue at (0, 0), fully transparent everywhere else.
    let mut pixels = vec![0u8; 4 * 4 * 4];
    pixels[0..4].copy_from_slice(&[0, 0, 255, 255]);
    let frame = Frame::new(4, 4, pixels, Duration::from_millis(100)).unwrap();
    let options = GifOptions {
        transparent_background: true,
        ..GifOptions::default()
    };

    let mut bytes = Vec::new();
    encode_gif(&mut bytes, &[frame], &options).unwrap();

    let mut decode = gif::DecodeOptions::new();
    decode.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = decode.read_info(Cursor::new(&bytes)).unwrap();
    let decoded = decoder.read_next_frame().unwrap().unwrap();
    assert_eq!(decoded.buffer[3], 255, "the opaque corner stays opaque");
    assert_eq!(decoded.buffer[2], 255, "and keeps its blue");
    // Pixel (1, 1) in a 4-pixel-wide image.
    let other = (4 + 1) * 4;
    assert_eq!(
        decoded.buffer[other + 3],
        0,
        "a transparent pixel decodes with alpha 0"
    );
}

#[test]
fn apng_round_trip_three_frames_delays_and_alpha() {
    let mut frames = three_frames();
    // Give the middle frame a partial-alpha pixel: straight red at alpha 100, premultiplied.
    let mut pixels = frames[1].pixels().to_vec();
    let i = ((5 * W + 5) * 4) as usize;
    pixels[i..i + 4].copy_from_slice(&[78, 0, 0, 100]);
    frames[1] = Frame::new(W, H, pixels, frames[1].duration()).unwrap();

    let mut bytes = Vec::new();
    encode_apng(&mut bytes, &frames, &ApngOptions::default()).unwrap();

    let mut reader = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
    let control = reader.info().animation_control().expect("file is animated");
    assert_eq!(control.num_frames, 3);
    assert_eq!(control.num_plays, 0, "0 plays means forever");

    let mut delays_ms = Vec::new();
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
    let mut partial_alpha = None;
    for index in 0..3 {
        reader.next_frame(&mut buffer).unwrap();
        let (numerator, denominator, width, height) = {
            let fc = reader
                .info()
                .frame_control()
                .expect("frame control for each frame");
            (fc.delay_num, fc.delay_den, fc.width, fc.height)
        };
        assert_eq!((width, height), (W, H));
        delays_ms.push(f64::from(numerator) * 1000.0 / f64::from(denominator));
        if index == 1 {
            let i = ((5 * W + 5) * 4) as usize;
            partial_alpha = Some((buffer[i], buffer[i + 3]));
        }
    }
    for delay in &delays_ms {
        assert!(
            (delay - 66.67).abs() < 0.5,
            "delay {delay} ms, expected 1000/15"
        );
    }
    let (red, alpha) = partial_alpha.unwrap();
    assert_eq!(alpha, 100, "partial alpha survives (GIF would drop it)");
    assert!(
        (i32::from(red) - 255).abs() <= 3 || (i32::from(red) - 200).abs() <= 3,
        "straight red {red}"
    );
}
