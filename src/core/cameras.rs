use crate::db::cameras::Camera;
use anyhow::{anyhow, Context, Result};
use ffmpeg::{codec, encoder, format, media, software, util, Dictionary, Rational};
use ffmpeg_next as ffmpeg;
use image::codecs::jpeg::JpegEncoder;
use image::ColorType;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

static FFMPEG_INIT: Once = Once::new();
static CAMERA_JOBS: OnceLock<Arc<Semaphore>> = OnceLock::new();
const MAX_CAMERA_JOBS: usize = 2;

pub async fn capture_snapshot(camera: &Camera) -> Result<Vec<u8>> {
    if let Some(snapshot_url) = camera.snapshot_url.as_deref() {
        return fetch_snapshot(snapshot_url).await;
    }

    let camera = camera.clone();
    run_camera_job(Duration::from_secs(20), move || {
        capture_snapshot_blocking(&camera)
    })
    .await
}

async fn fetch_snapshot(snapshot_url: &str) -> Result<Vec<u8>> {
    let response = reqwest::get(snapshot_url)
        .await
        .with_context(|| format!("Не удалось запросить snapshot URL: {}", snapshot_url))?;

    if !response.status().is_success() {
        return Err(anyhow!("Snapshot URL вернул статус {}", response.status()));
    }

    Ok(response.bytes().await?.to_vec())
}

pub async fn capture_clip(camera: &Camera, seconds: u32) -> Result<Vec<u8>> {
    let camera = camera.clone();
    let timeout = Duration::from_secs(seconds as u64 + 25);

    run_camera_job(timeout, move || capture_clip_blocking(&camera, seconds)).await
}

async fn run_camera_job<T, F>(timeout: Duration, job: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let semaphore = CAMERA_JOBS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_CAMERA_JOBS)))
        .clone();
    let permit = tokio::time::timeout(Duration::from_secs(2), semaphore.acquire_owned())
        .await
        .context("Очередь задач камеры занята")?
        .context("Очередь задач камеры закрыта")?;
    let (tx, rx) = tokio::sync::oneshot::channel();

    thread::Builder::new()
        .name("telegram-ha-camera".to_string())
        .spawn(move || {
            let result = job();
            let _ = tx.send(result);
            drop(permit);
        })
        .context("Не удалось запустить поток обработки камеры")?;

    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("Поток обработки камеры завершился без результата")),
        Err(_) => Err(anyhow!(
            "LibAV не успел ответить за {}с. Задача изолирована от tokio runtime и завершится сама, когда системный вызов вернется.",
            timeout.as_secs()
        )),
    }
}

fn init_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg init failed");
        format::network::init();
        ffmpeg::log::set_level(ffmpeg::log::Level::Warning);
    });
}

fn capture_snapshot_blocking(camera: &Camera) -> Result<Vec<u8>> {
    init_ffmpeg();

    let mut input_ctx = open_camera_input(&camera.stream_url)?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В потоке камеры нет видео"))?;
    let video_stream_index = input_stream.index();

    let context_decoder = codec::context::Context::from_parameters(input_stream.parameters())
        .context("Не удалось создать decoder context")?;
    let mut decoder = context_decoder
        .decoder()
        .video()
        .context("Не удалось открыть video decoder")?;

    let mut scaler = software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        format::Pixel::RGB24,
        decoder.width(),
        decoder.height(),
        software::scaling::flag::Flags::BILINEAR,
    )
    .context("Не удалось создать scaler")?;

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != video_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Не удалось отправить packet в decoder")?;

        if let Some(jpeg) = receive_first_jpeg_frame(&mut decoder, &mut scaler)? {
            return Ok(jpeg);
        }
    }

    decoder.send_eof().ok();
    if let Some(jpeg) = receive_first_jpeg_frame(&mut decoder, &mut scaler)? {
        return Ok(jpeg);
    }

    Err(anyhow!("Не удалось получить кадр из видеопотока"))
}

fn receive_first_jpeg_frame(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut software::scaling::context::Context,
) -> Result<Option<Vec<u8>>> {
    let mut decoded = util::frame::video::Video::empty();

    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut rgb_frame = util::frame::video::Video::empty();
        scaler
            .run(&decoded, &mut rgb_frame)
            .context("Не удалось преобразовать кадр в RGB")?;

        return Ok(Some(encode_rgb_frame_as_jpeg(&rgb_frame)?));
    }

    Ok(None)
}

fn encode_rgb_frame_as_jpeg(frame: &util::frame::video::Video) -> Result<Vec<u8>> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let stride = frame.stride(0);
    let data = frame.data(0);
    let row_len = width * 3;
    let mut rgb = Vec::with_capacity(row_len * height);

    for row in 0..height {
        let start = row * stride;
        let end = start + row_len;
        rgb.extend_from_slice(&data[start..end]);
    }

    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 85);
    encoder
        .encode(&rgb, width as u32, height as u32, ColorType::Rgb8.into())
        .context("Не удалось закодировать кадр в JPEG")?;

    Ok(bytes)
}

fn capture_clip_blocking(camera: &Camera, seconds: u32) -> Result<Vec<u8>> {
    init_ffmpeg();

    let output_path = temp_media_path(camera.id, "mp4");
    let result = remux_camera_clip(camera, seconds, &output_path);
    read_temp_file(output_path, result)
}

fn remux_camera_clip(camera: &Camera, seconds: u32, output_path: &PathBuf) -> Result<()> {
    let mut input_ctx = open_camera_input(&camera.stream_url)?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В потоке камеры нет видео"))?;
    let input_stream_index = input_stream.index();
    let input_time_base = input_stream.time_base();
    let fallback_packet_duration =
        frame_duration_ticks(input_stream.avg_frame_rate(), input_time_base)
            .or_else(|| frame_duration_ticks(input_stream.rate(), input_time_base))
            .unwrap_or_else(|| {
                frame_duration_ticks(Rational::new(30, 1), input_time_base).unwrap_or(1)
            });

    let output_path = output_path.to_string_lossy().to_string();
    let mut output_ctx = format::output(&output_path).context("Не удалось открыть MP4 output")?;

    let output_stream_index = {
        let mut output_stream = output_ctx
            .add_stream(encoder::find(codec::Id::None))
            .context("Не удалось создать output stream")?;
        output_stream.set_parameters(input_stream.parameters());

        unsafe {
            (*output_stream.parameters().as_mut_ptr()).codec_tag = 0;
        }

        output_stream.index()
    };

    let mut header_options = Dictionary::new();
    header_options.set("movflags", "+faststart");
    output_ctx
        .write_header_with(header_options)
        .context("Не удалось записать MP4 header")?;

    let mut wrote_packets = 0usize;
    let mut next_synthetic_pts = 0i64;
    let mut started_on_keyframe = false;
    let mut record_started_at = None;

    for (stream, mut packet) in input_ctx.packets() {
        if stream.index() != input_stream_index {
            continue;
        }

        if !started_on_keyframe {
            if !packet.is_key() {
                continue;
            }

            started_on_keyframe = true;
            record_started_at = Some(Instant::now());
        } else if record_started_at
            .is_some_and(|started_at| started_at.elapsed() >= Duration::from_secs(seconds as u64))
        {
            break;
        }

        normalize_packet_timestamps(
            &mut packet,
            &mut next_synthetic_pts,
            fallback_packet_duration,
        );

        let output_stream = output_ctx
            .stream(output_stream_index)
            .ok_or_else(|| anyhow!("Output stream missing"))?;
        packet.rescale_ts(input_time_base, output_stream.time_base());
        packet.set_position(-1);
        packet.set_stream(output_stream_index);
        packet
            .write_interleaved(&mut output_ctx)
            .context("Не удалось записать video packet в MP4")?;
        wrote_packets += 1;
    }

    if wrote_packets == 0 {
        return Err(anyhow!("Не удалось получить video packets из камеры"));
    }

    output_ctx
        .write_trailer()
        .context("Не удалось записать MP4 trailer")?;

    Ok(())
}

fn normalize_packet_timestamps(
    packet: &mut ffmpeg::Packet,
    next_synthetic_pts: &mut i64,
    fallback_duration: i64,
) {
    let duration = if packet.duration() > 0 {
        packet.duration()
    } else {
        fallback_duration
    };

    match (packet.pts(), packet.dts()) {
        (Some(pts), Some(dts)) => {
            *next_synthetic_pts = pts.max(dts).saturating_add(duration);
        }
        (Some(pts), None) => {
            packet.set_dts(Some(pts));
            *next_synthetic_pts = pts.saturating_add(duration);
        }
        (None, Some(dts)) => {
            packet.set_pts(Some(dts));
            *next_synthetic_pts = dts.saturating_add(duration);
        }
        (None, None) => {
            let pts = *next_synthetic_pts;
            packet.set_pts(Some(pts));
            packet.set_dts(Some(pts));
            *next_synthetic_pts = next_synthetic_pts.saturating_add(duration);
        }
    }

    if packet.duration() <= 0 {
        packet.set_duration(duration);
    }
}

fn frame_duration_ticks(frame_rate: Rational, time_base: Rational) -> Option<i64> {
    if frame_rate.numerator() <= 0
        || frame_rate.denominator() <= 0
        || time_base.numerator() <= 0
        || time_base.denominator() <= 0
    {
        return None;
    }

    let numerator = i64::from(frame_rate.denominator()) * i64::from(time_base.denominator());
    let denominator = i64::from(frame_rate.numerator()) * i64::from(time_base.numerator());

    Some((numerator / denominator).max(1))
}

fn open_camera_input(stream_url: &str) -> Result<format::context::Input> {
    let mut options = Dictionary::new();

    if stream_url.starts_with("rtsp://") || stream_url.starts_with("rtsps://") {
        options.set("rtsp_transport", "tcp");
    }

    options.set("timeout", "5000000");
    options.set("stimeout", "5000000");

    format::input_with_dictionary(stream_url, options)
        .with_context(|| format!("Не удалось открыть видеопоток {}", stream_url))
}

fn read_temp_file(path: PathBuf, command_result: Result<()>) -> Result<Vec<u8>> {
    if let Err(error) = command_result {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }

    let bytes =
        std::fs::read(&path).with_context(|| format!("Не удалось прочитать файл {:?}", path))?;
    let _ = std::fs::remove_file(&path);

    if bytes.is_empty() {
        return Err(anyhow!("LibAV создал пустой файл"));
    }

    Ok(bytes)
}

fn temp_media_path(camera_id: i64, extension: &str) -> PathBuf {
    let millis = chrono::Utc::now().timestamp_millis();
    std::env::temp_dir().join(format!(
        "telegram_ha_bot_camera_{}_{}.{}",
        camera_id, millis, extension
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_duration_uses_stream_time_base() {
        assert_eq!(
            frame_duration_ticks(Rational::new(30, 1), Rational::new(1, 90_000)),
            Some(3_000)
        );
    }

    #[test]
    fn frame_duration_rejects_invalid_rate() {
        assert_eq!(
            frame_duration_ticks(Rational::new(0, 1), Rational::new(1, 90_000)),
            None
        );
    }
}
