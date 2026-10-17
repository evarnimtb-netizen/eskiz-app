//! Adapter selection.
use eskiz_gpu::{AdapterKind, GpuContext, GpuError, GpuOptions};

#[test]
fn default_context_prefers_hardware() {
    match GpuContext::new() {
        Ok(ctx) => {
            eprintln!("[context] {}", ctx.describe());
            // If any hardware adapter works, the default must not have picked software.
            let hw = GpuContext::with_options(GpuOptions {
                allow_software: false,
                force_software: false,
            });
            if hw.is_ok() {
                assert_eq!(ctx.kind, AdapterKind::Hardware);
            }
            assert!(ctx.describe().contains(&ctx.info.name));
        }
        Err(e) => {
            // No adapter at all: the error must say so clearly.
            assert!(matches!(e, GpuError::Unavailable(_) | GpuError::Device(_)), "{e}");
            eprintln!("[context] SKIPPED: no GPU adapter available ({e})");
        }
    }
}

#[test]
fn forcing_software_never_returns_hardware() {
    match GpuContext::with_options(GpuOptions {
        allow_software: true,
        force_software: true,
    }) {
        Ok(ctx) => assert_eq!(ctx.kind, AdapterKind::Software),
        // Machines without a software rasteriser (macOS has none) report Unavailable.
        Err(e) => assert!(matches!(e, GpuError::Unavailable(_) | GpuError::Device(_)), "{e}"),
    }
}
