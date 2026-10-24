//! The byte-backed transform session against the float one it replaces in the app: the same
//! pixels from the same layer, selection and mapping, with and without a warp mesh.

use eskiz_select::{Image, Interp, Mask, Mat3, Point, TransformSession};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A valid premultiplied layer: transparent margin, then opaque, translucent and empty pixels.
fn layer(seed: u64, w: usize, h: usize) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            if x < 12 || y < 9 || x + 15 > w || y + 11 > h {
                continue;
            }
            let a = [255u8, 255, 0, (rng.next() % 256) as u8][(rng.next() % 4) as usize];
            let p = &mut out[(y * w + x) * 4..][..4];
            for c in p.iter_mut().take(3) {
                *c = if a == 0 { 0 } else { (rng.next() % (u64::from(a) + 1)) as u8 };
            }
            p[3] = a;
        }
    }
    out
}

/// A selection with whole, empty and fractional values (a feathered edge).
fn selection(seed: u64, w: usize, h: usize) -> Mask {
    let mut rng = Rng(seed | 1);
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if x < 20 || y < 15 || x > w - 25 {
                0.0
            } else {
                [1.0, 1.0, 0.5, rng.unit()][(rng.next() % 4) as usize]
            }
        })
        .collect();
    Mask { width: w, height: h, data }
}

fn mappings(w: usize, h: usize) -> Vec<Mat3> {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let about = |m: Mat3| Mat3::translate(cx, cy) * m * Mat3::translate(-cx, -cy);
    vec![
        Mat3::IDENTITY,
        Mat3::translate(13.4, -7.1),
        about(Mat3::rotate_deg(23.0) * Mat3::scale(1.1, 1.1)),
        about(Mat3::rotate_deg(-110.0) * Mat3::scale(0.6, 0.9)),
        about(Mat3::scale(-1.2, 1.0)),
        Mat3 { m: [[1.0, 0.1, 3.0], [0.05, 0.9, 1.0], [0.0006, 0.0003, 1.0]] },
    ]
}

/// The float session as the app built it before: straight float image, selection multiplied
/// into its alpha, the selection as the footprint.
fn float_session(layer: &[u8], sel: Option<&Mask>, w: usize, h: usize, interp: Interp) -> TransformSession {
    let mut source = Image::from_premul_rgba8(w, h, layer).expect("size");
    let footprint = match sel {
        Some(m) => {
            for (px, &s) in source.pixels.iter_mut().zip(&m.data) {
                px[3] *= s;
            }
            m.clone()
        }
        None => Mask::full(w, h),
    };
    TransformSession::new(source, footprint, Mat3::IDENTITY, interp).expect("session")
}

fn bytes_session(layer: &[u8], sel: Option<&Mask>, w: usize, h: usize, interp: Interp) -> TransformSession {
    TransformSession::new_bytes(w, h, layer.to_vec(), sel.cloned(), Mat3::IDENTITY, interp).expect("session")
}

fn shake_mesh(s: &mut TransformSession) {
    s.enter_warp().expect("warp");
    let mut rng = Rng(77);
    let mesh = s.warp_mut().expect("mesh");
    for column in mesh.ctrl.iter_mut() {
        for p in column.iter_mut() {
            *p = Point::new(p.x + (rng.unit() - 0.5) * 14.0, p.y + (rng.unit() - 0.5) * 14.0);
        }
    }
}

#[test]
fn commit_premul_equals_the_float_commit() {
    let (w, h) = (170, 130);
    let l = layer(3, w, h);
    let sel = selection(4, w, h);
    let base = Image::from_premul_rgba8(w, h, &l).expect("size");
    for selected in [false, true] {
        let sel = selected.then_some(&sel);
        for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
            for (i, m) in mappings(w, h).into_iter().enumerate() {
                let mut a = float_session(&l, sel, w, h, interp);
                let mut b = bytes_session(&l, sel, w, h, interp);
                a.set_mapping(m).expect("mapping");
                b.set_mapping(m).expect("mapping");
                let want = a.commit(&base).expect("commit").to_premul_rgba8();
                let got = b.commit_premul(Some(&l)).expect("commit");
                assert!(got == want, "mapping {i} {interp:?} selection {selected}");
                if !selected {
                    // Without a selection the layer pixels are not needed at all.
                    assert!(b.commit_premul(None).expect("commit") == want);
                }
            }
        }
    }
}

#[test]
fn commit_premul_equals_the_float_commit_in_warp_mode() {
    let (w, h) = (170, 130);
    let l = layer(5, w, h);
    let sel = selection(6, w, h);
    let base = Image::from_premul_rgba8(w, h, &l).expect("size");
    for selected in [false, true] {
        let sel = selected.then_some(&sel);
        for interp in [Interp::Bilinear, Interp::Bicubic] {
            let mut a = float_session(&l, sel, w, h, interp);
            let mut b = bytes_session(&l, sel, w, h, interp);
            shake_mesh(&mut a);
            shake_mesh(&mut b);
            let want = a.commit(&base).expect("commit").to_premul_rgba8();
            assert!(b.commit_premul(Some(&l)).expect("commit") == want, "warp {interp:?} selection {selected}");
        }
    }
}

#[test]
fn render_and_marquee_equal_the_float_session() {
    let (w, h) = (170, 130);
    let l = layer(7, w, h);
    let sel = selection(8, w, h);
    for m in mappings(w, h) {
        let mut a = float_session(&l, Some(&sel), w, h, Interp::Bicubic);
        let mut b = bytes_session(&l, Some(&sel), w, h, Interp::Bicubic);
        a.set_mapping(m).expect("mapping");
        b.set_mapping(m).expect("mapping");
        assert!(a.render() == b.render());
        assert!(a.marquee(&sel) == b.marquee(&sel));
        assert_eq!(a.source_rect(), b.source_rect());
    }
}

/// A smooth layer, so block averaging and resampling order hardly matter.
fn smooth_layer(w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for y in 20..h - 20 {
        for x in 30..w - 30 {
            let p = &mut out[(y * w + x) * 4..][..4];
            let (r, g, b) = ((x * 255 / w) as u8, (y * 255 / h) as u8, 90u8);
            p.copy_from_slice(&[r / 2, g / 2, b / 2, 128 + (x * 100 / w) as u8]);
        }
    }
    out
}

#[test]
fn preview_premul_at_full_size_matches_the_bilinear_commit() {
    let (w, h) = (170, 130);
    let l = smooth_layer(w, h);
    let mut s = bytes_session(&l, None, w, h, Interp::Bicubic);
    s.set_mapping(mappings(w, h)[2]).expect("mapping");
    let (pw, ph, got) = s.preview_premul(1, Interp::Bilinear).expect("preview");
    assert_eq!((pw, ph), (w, h));
    let mut reference = bytes_session(&l, None, w, h, Interp::Bilinear);
    reference.set_mapping(mappings(w, h)[2]).expect("mapping");
    let want = reference.commit_premul(None).expect("commit");
    let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(worst <= 1, "worst difference {worst}");
}

#[test]
fn preview_premul_at_reduced_size_follows_the_commit() {
    let (w, h) = (400, 300);
    let l = smooth_layer(w, h);
    let m = mappings(w, h)[2];
    for k in [2usize, 4] {
        let mut s = bytes_session(&l, None, w, h, Interp::Bicubic);
        s.set_mapping(m).expect("mapping");
        let (pw, ph, got) = s.preview_premul(k, Interp::Bilinear).expect("preview");
        assert_eq!((pw, ph), (w.div_ceil(k), h.div_ceil(k)));
        let full = s.commit_premul(None).expect("commit");
        // The commit averaged over k x k blocks is what the preview should show.
        let (mut sum, mut n, mut worst) = (0u64, 0u64, 0u8);
        for y in 0..ph {
            for x in 0..pw {
                for c in 0..4 {
                    let mut acc = 0u32;
                    let mut cnt = 0u32;
                    for yy in y * k..((y + 1) * k).min(h) {
                        for xx in x * k..((x + 1) * k).min(w) {
                            acc += u32::from(full[(yy * w + xx) * 4 + c]);
                            cnt += 1;
                        }
                    }
                    let d = ((acc as f32 / cnt as f32).round() as i32 - i32::from(got[(y * pw + x) * 4 + c])).unsigned_abs() as u8;
                    sum += u64::from(d);
                    n += 1;
                    worst = worst.max(d);
                }
            }
        }
        let mean = sum as f64 / n as f64;
        assert!(mean < 1.5, "k{k}: mean difference {mean}");
        assert!(worst < 70, "k{k}: worst difference {worst} (edge pixels)");
    }
}

#[test]
fn wrong_sizes_and_empty_layers_are_refused() {
    assert!(TransformSession::new_bytes(4, 4, vec![0; 10], None, Mat3::IDENTITY, Interp::Bicubic).is_err());
    assert!(TransformSession::new_bytes(4, 4, vec![0; 64], None, Mat3::IDENTITY, Interp::Bicubic).is_err(), "nothing visible");
    let m = Mask::full(3, 3);
    assert!(TransformSession::new_bytes(4, 4, vec![255; 64], Some(m), Mat3::IDENTITY, Interp::Bicubic).is_err());
    let s = TransformSession::new_bytes(4, 4, vec![255; 64], None, Mat3::IDENTITY, Interp::Bicubic).expect("session");
    assert!(s.commit_premul(Some(&[0; 3])).is_err());
}

/// Timing probe (run with `--ignored --nocapture`): a full-layer bicubic commit.
#[test]
#[ignore]
fn probe_commit_speed() {
    let (w, h) = (2048usize, 2048usize);
    let mut l = vec![0u8; w * h * 4];
    let mut rng = Rng(1);
    for p in l.as_chunks_mut::<4>().0 {
        let v = (rng.next() % 256) as u8;
        *p = [v, v / 2, v / 3, 255];
    }
    let mut s = bytes_session(&l, None, w, h, Interp::Bicubic);
    s.set_mapping(Mat3::translate(1024.0, 1024.0) * Mat3::rotate_deg(15.0) * Mat3::scale(1.1, 1.1) * Mat3::translate(-1024.0, -1024.0)).expect("mapping");
    for interp in [Interp::Bicubic, Interp::Bilinear] {
        let mut s2 = bytes_session(&l, None, w, h, interp);
        s2.set_mapping(Mat3::translate(1024.0, 1024.0) * Mat3::rotate_deg(15.0) * Mat3::scale(1.1, 1.1) * Mat3::translate(-1024.0, -1024.0)).expect("mapping");
        let t = std::time::Instant::now();
        let out = s2.commit_premul(None).expect("commit");
        eprintln!("{interp:?}: {:?} ({} bytes)", t.elapsed(), out.len());
    }
    let _ = &mut s;
}

/// A transform that moves nothing gives the layer back, even along a soft (feathered) selection
/// edge: the lifted content fills its own hole instead of going over what is left.
#[test]
fn an_identity_transform_of_a_soft_selection_changes_nothing() {
    let (w, h) = (24, 16);
    let opaque: Vec<u8> = (0..w * h).flat_map(|i| [(i % 200) as u8, 90, 40, 255]).collect();
    let mut sel = Mask::empty(w, h);
    for (i, s) in sel.data.iter_mut().enumerate() {
        *s = (i % w) as f32 / (w - 1) as f32; // a ramp from 0 to 1 across the layer
    }
    let s = TransformSession::new_bytes(w, h, opaque.clone(), Some(sel), Mat3::IDENTITY, Interp::Nearest).expect("session");
    let out = s.commit_premul(Some(&opaque)).expect("commit");
    for (got, want) in out.as_chunks::<4>().0.iter().zip(opaque.as_chunks::<4>().0) {
        for c in 0..4 {
            assert!(got[c].abs_diff(want[c]) <= 1, "{got:?} vs {want:?}");
        }
    }
}
