use anyhow::{Context as _, Result};
use ffmpeg_next::{ffi, format::Pixel, frame, software::scaling, util::color};

/// YUVJ formats share their plane layout with YUV; their distinction is full range.
fn normalized_format(format: Pixel) -> Pixel {
    match format {
        Pixel::YUVJ420P => Pixel::YUV420P,
        Pixel::YUVJ422P => Pixel::YUV422P,
        Pixel::YUVJ444P => Pixel::YUV444P,
        Pixel::YUVJ440P => Pixel::YUV440P,
        Pixel::YUVJ411P => Pixel::YUV411P,
        other => other,
    }
}

struct NativeContext(std::ptr::NonNull<ffi::SwsContext>);

impl Drop for NativeContext {
    fn drop(&mut self) {
        // SAFETY: this context has a single owner, including initialization failures.
        unsafe { ffi::sws_freeContext(self.0.as_ptr()) };
    }
}

pub(super) struct FrameScaler {
    input: scaling::context::Definition,
    output: scaling::context::Definition,
    context: Option<(bool, color::Space, NativeContext)>,
}

fn check(result: i32) -> Result<()> {
    if result < 0 {
        Err(ffmpeg_next::Error::from(result).into())
    } else {
        Ok(())
    }
}

impl FrameScaler {
    pub(super) fn new(
        source: Pixel,
        source_width: u32,
        source_height: u32,
        output: Pixel,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        anyhow::ensure!(
            source_width > 0 && source_height > 0 && width > 0 && height > 0,
            "Нулевой размер кадра"
        );
        Ok(Self {
            input: scaling::context::Definition {
                format: normalized_format(source),
                width: source_width,
                height: source_height,
            },
            output: scaling::context::Definition {
                format: output,
                width,
                height,
            },
            context: None,
        })
    }

    fn configure(&mut self, full: bool, space: color::Space) -> Result<()> {
        if self
            .context
            .as_ref()
            .is_some_and(|(range, matrix, _)| *range == full && *matrix == space)
        {
            return Ok(());
        }
        // SAFETY: allocation is checked and immediately owned; all option names
        // are static C strings. The legacy option API also works with FFmpeg 5.x
        // on Debian. Ranges must be set BEFORE init selects a YUV copy/scale path.
        let context = unsafe {
            let ptr = std::ptr::NonNull::new(ffi::sws_alloc_context())
                .context("Не удалось выделить scaler")?;
            let context = NativeContext(ptr);
            let source_format: ffi::AVPixelFormat = self.input.format.into();
            let output_format: ffi::AVPixelFormat = self.output.format.into();
            for (name, value) in [
                (c"srcw", i64::from(self.input.width)),
                (c"srch", i64::from(self.input.height)),
                (c"src_format", source_format as i64),
                (c"dstw", i64::from(self.output.width)),
                (c"dsth", i64::from(self.output.height)),
                (c"dst_format", output_format as i64),
                (c"sws_flags", i64::from(scaling::Flags::BILINEAR.bits())),
                (c"src_range", i64::from(full)),
                (c"dst_range", i64::from(self.output.format == Pixel::RGB24)),
            ] {
                check(ffi::av_opt_set_int(
                    ptr.as_ptr().cast(),
                    name.as_ptr(),
                    value,
                    0,
                ))?;
            }
            check(ffi::sws_init_context(
                ptr.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ))?;
            let matrix = match space {
                color::Space::BT709 => ffi::SWS_CS_ITU709,
                color::Space::FCC => ffi::SWS_CS_FCC,
                color::Space::SMPTE240M => ffi::SWS_CS_SMPTE240M,
                color::Space::BT2020NCL | color::Space::BT2020CL => ffi::SWS_CS_BT2020,
                _ => ffi::SWS_CS_DEFAULT,
            };
            let coefficients = ffi::sws_getCoefficients(matrix);
            check(ffi::sws_setColorspaceDetails(
                ptr.as_ptr(),
                coefficients,
                i32::from(full),
                coefficients,
                i32::from(self.output.format == Pixel::RGB24),
                0,
                1 << 16,
                1 << 16,
            ))?;
            context
        };
        self.context = Some((full, space, context));
        Ok(())
    }

    pub(super) fn run(&mut self, input: &frame::Video, output: &mut frame::Video) -> Result<()> {
        let normalized = normalized_format(input.format());
        if normalized != self.input.format
            || input.width() != self.input.width
            || input.height() != self.input.height
        {
            return Err(ffmpeg_next::Error::InputChanged.into());
        }
        let full = normalized != input.format() || input.color_range() == color::Range::JPEG;
        self.configure(full, input.color_space())?;
        // SAFETY: is_empty only reads the live frame header; allocation owns its buffers.
        if unsafe { output.is_empty() } {
            *output = frame::Video::new(self.output.format, self.output.width, self.output.height);
        }
        if output.format() != self.output.format
            || output.width() != self.output.width
            || output.height() != self.output.height
        {
            return Err(ffmpeg_next::Error::OutputChanged.into());
        }
        // SAFETY: dimensions and layouts have been checked above. YUVJ and YUV
        // aliases use identical planes and strides. Both frames own their buffers
        // for this call; sws_scale does not retain the plane pointers.
        let rows = unsafe {
            ffi::sws_scale(
                self.context.as_ref().unwrap().2 .0.as_ptr(),
                (*input.as_ptr()).data.as_ptr().cast(),
                (*input.as_ptr()).linesize.as_ptr(),
                0,
                self.input.height as i32,
                (*output.as_mut_ptr()).data.as_ptr(),
                (*output.as_mut_ptr()).linesize.as_ptr(),
            )
        };
        check(rows)?;
        anyhow::ensure!(
            rows == self.output.height as i32,
            "Не полный преобразованный кадр"
        );
        let rgb = self.output.format == Pixel::RGB24;
        output.set_color_range(if rgb {
            color::Range::JPEG
        } else {
            color::Range::MPEG
        });
        output.set_color_space(if rgb {
            color::Space::RGB
        } else {
            input.color_space()
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray_bands(format: Pixel, range: color::Range) -> frame::Video {
        let mut frame = frame::Video::new(format, 160, 16);
        frame.set_color_range(range);
        frame.set_color_space(color::Space::BT709);
        let stride = frame.stride(0);
        for y in 0..16 {
            for x in 0..160 {
                frame.data_mut(0)[y * stride + x] = [0, 16, 128, 235, 255][x / 32];
            }
        }
        frame.data_mut(1).fill(128);
        frame.data_mut(2).fill(128);
        frame
    }

    #[test]
    fn full_range_formats_preserve_gray_levels_in_rgb() -> Result<()> {
        super::super::init_ffmpeg();
        for format in [
            Pixel::YUVJ420P,
            Pixel::YUVJ422P,
            Pixel::YUVJ444P,
            Pixel::YUVJ440P,
            Pixel::YUVJ411P,
            Pixel::YUV420P,
        ] {
            let source = gray_bands(format, color::Range::JPEG);
            let mut scaler = FrameScaler::new(format, 160, 16, Pixel::RGB24, 160, 16)?;
            let mut rgb = frame::Video::empty();
            scaler.run(&source, &mut rgb)?;
            assert_eq!(source.format(), format);
            for (band, expected) in [0_i16, 16, 128, 235, 255].into_iter().enumerate() {
                for component in &rgb.data(0)[(band * 32 + 16) * 3..(band * 32 + 16) * 3 + 3] {
                    assert!(
                        (i16::from(*component) - expected).abs() <= 2,
                        "{format:?}: {component} != {expected}"
                    );
                }
            }
            assert_eq!(rgb.color_range(), color::Range::JPEG);
        }
        Ok(())
    }

    #[test]
    fn scaler_honors_range_changes_between_frames() -> Result<()> {
        super::super::init_ffmpeg();
        let mut scaler = FrameScaler::new(Pixel::YUV420P, 160, 16, Pixel::RGB24, 160, 16)?;
        for range in [
            color::Range::JPEG,
            color::Range::MPEG,
            color::Range::Unspecified,
            color::Range::JPEG,
        ] {
            let source = gray_bands(Pixel::YUV420P, range);
            let mut rgb = frame::Video::empty();
            scaler.run(&source, &mut rgb)?;
            let black = rgb.data(0)[48 * 3];
            let white = rgb.data(0)[112 * 3];
            let (expected_black, expected_white) = if range == color::Range::JPEG {
                (16, 235)
            } else {
                (0, 255)
            };
            assert!((i16::from(black) - expected_black).abs() <= 2);
            assert!((i16::from(white) - expected_white).abs() <= 2);
        }
        Ok(())
    }

    #[test]
    fn legacy_full_range_is_converted_to_limited_range_for_h264() -> Result<()> {
        super::super::init_ffmpeg();
        let source = gray_bands(Pixel::YUVJ420P, color::Range::Unspecified);
        let mut scaler = FrameScaler::new(source.format(), 160, 16, Pixel::YUV420P, 160, 16)?;
        let mut output = frame::Video::empty();
        scaler.run(&source, &mut output)?;
        for (band, expected) in [16_i16, 30, 126, 218, 235].into_iter().enumerate() {
            assert!((i16::from(output.data(0)[band * 32 + 16]) - expected).abs() <= 2);
        }
        assert_eq!(output.color_range(), color::Range::MPEG);
        assert_eq!(output.color_space(), color::Space::BT709);
        assert_eq!(source.format(), Pixel::YUVJ420P);
        Ok(())
    }
}
