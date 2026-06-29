use crate::db::cameras::Camera;
use anyhow::{anyhow, Context, Result};
use ffmpeg::{codec, encoder, format, frame, media, picture, software, util, Dictionary, Rational};
use ffmpeg_next as ffmpeg;
use image::codecs::jpeg::JpegEncoder;
use image::ColorType;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

static FFMPEG_INIT: Once = Once::new();
static LIVE_CAMERA_JOBS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static LOCAL_MEDIA_JOBS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static TIMED_OUT_LIVE_CAMERA_JOBS: AtomicUsize = AtomicUsize::new(0);
static TIMED_OUT_LOCAL_MEDIA_JOBS: AtomicUsize = AtomicUsize::new(0);
const MAX_LIVE_CAMERA_JOBS: usize = 2;
const MAX_LOCAL_MEDIA_JOBS: usize = 2;
const MAX_TIMED_OUT_LIVE_CAMERA_JOBS: usize = 2;
const MAX_TIMED_OUT_LOCAL_MEDIA_JOBS: usize = 2;
const REMUX_KEYFRAME_WAIT_S: u64 = 10;
const SNAPSHOT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct CameraJobState {
    done: bool,
    timed_out: bool,
}

#[derive(Clone, Copy)]
enum CameraJobPool {
    LiveCamera,
    LocalMedia,
}

impl CameraJobPool {
    fn semaphore(self) -> Arc<Semaphore> {
        match self {
            Self::LiveCamera => LIVE_CAMERA_JOBS
                .get_or_init(|| Arc::new(Semaphore::new(MAX_LIVE_CAMERA_JOBS)))
                .clone(),
            Self::LocalMedia => LOCAL_MEDIA_JOBS
                .get_or_init(|| Arc::new(Semaphore::new(MAX_LOCAL_MEDIA_JOBS)))
                .clone(),
        }
    }

    fn timed_out_jobs(self) -> &'static AtomicUsize {
        match self {
            Self::LiveCamera => &TIMED_OUT_LIVE_CAMERA_JOBS,
            Self::LocalMedia => &TIMED_OUT_LOCAL_MEDIA_JOBS,
        }
    }

    fn max_timed_out_jobs(self) -> usize {
        match self {
            Self::LiveCamera => MAX_TIMED_OUT_LIVE_CAMERA_JOBS,
            Self::LocalMedia => MAX_TIMED_OUT_LOCAL_MEDIA_JOBS,
        }
    }

    fn queue_name(self) -> &'static str {
        match self {
            Self::LiveCamera => "камеры",
            Self::LocalMedia => "локального видео",
        }
    }

    fn unavailable_message(self) -> &'static str {
        match self {
            Self::LiveCamera => {
                "Камеры временно недоступны: есть зависшие LibAV-задачи после таймаута. Перезапустите сервис, если поток камеры не восстановится."
            }
            Self::LocalMedia => {
                "Видеоархив временно недоступен: есть зависшие LibAV-задачи обработки локального видео. Перезапустите сервис, если обработка не восстановится."
            }
        }
    }

    fn thread_name(self) -> &'static str {
        match self {
            Self::LiveCamera => "telegram-ha-camera",
            Self::LocalMedia => "telegram-ha-media",
        }
    }
}

pub async fn capture_snapshot(camera: &Camera) -> Result<Vec<u8>> {
    if let Some(snapshot_url) = camera.snapshot_url.as_deref() {
        return fetch_snapshot(snapshot_url).await;
    }

    let camera = camera.clone();
    run_isolated_job(
        CameraJobPool::LiveCamera,
        Duration::from_secs(20),
        move || capture_snapshot_blocking(&camera),
    )
    .await
}

async fn fetch_snapshot(snapshot_url: &str) -> Result<Vec<u8>> {
    let safe_url = safe_stream_label(snapshot_url);
    match tokio::time::timeout(
        SNAPSHOT_HTTP_TIMEOUT,
        fetch_snapshot_inner(snapshot_url, &safe_url),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "Snapshot URL {} не ответил за {}с",
            safe_url,
            SNAPSHOT_HTTP_TIMEOUT.as_secs()
        )),
    }
}

async fn fetch_snapshot_inner(snapshot_url: &str, safe_url: &str) -> Result<Vec<u8>> {
    let response = reqwest::get(snapshot_url).await.map_err(|error| {
        anyhow!(
            "Не удалось запросить snapshot URL {}: {}",
            safe_url,
            sanitize_camera_error_text(&error.to_string())
        )
    })?;

    if !response.status().is_success() {
        return Err(anyhow!("Snapshot URL вернул статус {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| {
            anyhow!(
                "Не удалось прочитать snapshot response {}: {}",
                safe_url,
                sanitize_camera_error_text(&error.to_string())
            )
        })?
        .to_vec();
    if bytes.is_empty() {
        return Err(anyhow!("Snapshot URL вернул пустой файл"));
    }

    Ok(bytes)
}

pub async fn capture_clip(camera: &Camera, seconds: u32) -> Result<Vec<u8>> {
    let camera = camera.clone();
    let timeout = Duration::from_secs(seconds as u64 + 45);

    log::info!(
        "Camera clip capture requested: camera={} {}, duration={}s, timeout={}s, stream={}",
        camera.id,
        camera.name,
        seconds,
        timeout.as_secs(),
        safe_stream_label(&camera.stream_url)
    );
    run_isolated_job(CameraJobPool::LiveCamera, timeout, move || {
        capture_clip_blocking(&camera, seconds)
    })
    .await
}

pub async fn capture_clip_to_file(
    camera: &Camera,
    seconds: u32,
    output_path: PathBuf,
) -> Result<u64> {
    let camera = camera.clone();
    let timeout = Duration::from_secs(seconds as u64 + 45);

    log::info!(
        "Camera clip file capture requested: camera={} {}, duration={}s, timeout={}s, stream={}, output={}",
        camera.id,
        camera.name,
        seconds,
        timeout.as_secs(),
        safe_stream_label(&camera.stream_url),
        output_path.display()
    );
    run_isolated_job(CameraJobPool::LiveCamera, timeout, move || {
        write_clip_file_blocking(&camera, seconds, &output_path)
    })
    .await
}

pub async fn compress_clip_for_telegram(
    input_path: PathBuf,
    output_path: PathBuf,
    max_bytes: u64,
) -> Result<()> {
    run_isolated_job(
        CameraJobPool::LocalMedia,
        Duration::from_secs(180),
        move || compress_clip_blocking(&input_path, &output_path, max_bytes),
    )
    .await
}

pub async fn extract_video_preview(input_path: PathBuf) -> Result<Vec<u8>> {
    run_isolated_job(
        CameraJobPool::LocalMedia,
        Duration::from_secs(30),
        move || extract_video_preview_blocking(&input_path),
    )
    .await
}

async fn run_isolated_job<T, F>(pool: CameraJobPool, timeout: Duration, job: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let timed_out_jobs = pool.timed_out_jobs();
    if timed_out_jobs.load(Ordering::Relaxed) >= pool.max_timed_out_jobs() {
        return Err(anyhow!(pool.unavailable_message()));
    }

    let semaphore = pool.semaphore();
    let permit = tokio::time::timeout(Duration::from_secs(2), semaphore.acquire_owned())
        .await
        .with_context(|| format!("Очередь задач {} занята", pool.queue_name()))?
        .with_context(|| format!("Очередь задач {} закрыта", pool.queue_name()))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let state = Arc::new(Mutex::new(CameraJobState::default()));
    let thread_state = state.clone();

    thread::Builder::new()
        .name(pool.thread_name().to_string())
        .spawn(move || {
            let result = job();
            let _ = tx.send(result);
            if let Ok(mut state) = thread_state.lock() {
                state.done = true;
                if state.timed_out {
                    timed_out_jobs.fetch_sub(1, Ordering::Relaxed);
                }
            }
        })
        .context("Не удалось запустить поток обработки камеры")?;

    let result = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("Поток обработки камеры завершился без результата")),
        Err(_) => {
            if let Ok(mut state) = state.lock() {
                if !state.done && !state.timed_out {
                    state.timed_out = true;
                    timed_out_jobs.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(anyhow!(
                "LibAV не успел ответить за {}с. Задача изолирована от tokio runtime и завершится сама, когда системный вызов вернется.",
                timeout.as_secs()
            ))
        }
    };

    drop(permit);
    result
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

fn extract_video_preview_blocking(input_path: &Path) -> Result<Vec<u8>> {
    init_ffmpeg();

    let input_path_str = input_path.to_string_lossy().to_string();
    let mut input_ctx = format::input(&input_path_str)
        .with_context(|| format!("Не удалось открыть видео {}", input_path.display()))?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В файле нет видеопотока"))?;
    let video_stream_index = input_stream.index();

    let context_decoder = codec::context::Context::from_parameters(input_stream.parameters())
        .context("Не удалось создать preview decoder context")?;
    let mut decoder = context_decoder
        .decoder()
        .video()
        .context("Не удалось открыть preview video decoder")?;

    let (target_width, target_height) =
        scaled_dimensions(decoder.width(), decoder.height(), 1280, 720);
    let mut scaler = software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        format::Pixel::RGB24,
        target_width,
        target_height,
        software::scaling::flag::Flags::BILINEAR,
    )
    .context("Не удалось создать preview scaler")?;

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != video_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Не удалось отправить packet в preview decoder")?;

        if let Some(jpeg) = receive_first_jpeg_frame(&mut decoder, &mut scaler)? {
            return Ok(jpeg);
        }
    }

    decoder.send_eof().ok();
    if let Some(jpeg) = receive_first_jpeg_frame(&mut decoder, &mut scaler)? {
        return Ok(jpeg);
    }

    Err(anyhow!("Не удалось получить preview кадр из видео"))
}

fn receive_first_jpeg_frame(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut software::scaling::context::Context,
) -> Result<Option<Vec<u8>>> {
    let mut decoded = util::frame::video::Video::empty();

    if decoder.receive_frame(&mut decoded).is_ok() {
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

    if bytes.is_empty() {
        return Err(anyhow!("LibAV закодировал пустой JPEG"));
    }

    Ok(bytes)
}

fn capture_clip_blocking(camera: &Camera, seconds: u32) -> Result<Vec<u8>> {
    let output_path = temp_media_path(camera.id, "mp4");
    let result = write_clip_file_blocking(camera, seconds, &output_path).map(|_| ());
    read_temp_file(output_path, result)
}

fn write_clip_file_blocking(camera: &Camera, seconds: u32, output_path: &Path) -> Result<u64> {
    init_ffmpeg();

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать папку {}", parent.display()))?;
    }

    let tmp_output = tmp_output_path(output_path);
    let _ = std::fs::remove_file(&tmp_output);
    let result = remux_camera_clip(camera, seconds, &tmp_output);
    match publish_media_file(&tmp_output, output_path, result) {
        Ok(size_bytes) => Ok(size_bytes),
        Err(remux_error) => {
            log::warn!(
                "Camera remux failed, trying transcode fallback: camera={} {}, error={}",
                camera.id,
                camera.name,
                sanitize_camera_error_text(&remux_error.to_string())
            );

            let _ = std::fs::remove_file(&tmp_output);
            let fallback_result = transcode_camera_clip(
                camera,
                seconds,
                &tmp_output,
                CompressionProfile {
                    max_width: 1920,
                    max_height: 1080,
                    bit_rate: 4_000_000,
                    max_bit_rate: 5_000_000,
                    preset: "veryfast",
                },
            );
            publish_media_file(&tmp_output, output_path, fallback_result).with_context(|| {
                format!(
                    "remux failed before transcode fallback: {}",
                    sanitize_camera_error_text(&remux_error.to_string())
                )
            })
        }
    }
}

fn publish_media_file(
    tmp_path: &Path,
    output_path: &Path,
    command_result: Result<()>,
) -> Result<u64> {
    if let Err(error) = command_result {
        let _ = std::fs::remove_file(tmp_path);
        return Err(error);
    }

    let metadata = std::fs::metadata(tmp_path)
        .with_context(|| format!("Не удалось прочитать размер {}", tmp_path.display()))?;
    if !metadata.is_file() {
        let _ = std::fs::remove_file(tmp_path);
        return Err(anyhow!(
            "LibAV создал путь, который не является файлом: {}",
            tmp_path.display()
        ));
    }

    let size_bytes = metadata.len();
    if size_bytes == 0 {
        let _ = std::fs::remove_file(tmp_path);
        return Err(anyhow!("LibAV создал пустой файл"));
    }

    std::fs::rename(tmp_path, output_path)
        .with_context(|| format!("Не удалось опубликовать видео {}", output_path.display()))?;

    Ok(size_bytes)
}

fn compress_clip_blocking(input_path: &Path, output_path: &Path, max_bytes: u64) -> Result<()> {
    init_ffmpeg();

    let tmp_output = tmp_output_path(output_path);
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать папку {}", parent.display()))?;
    }

    let profiles = [
        CompressionProfile {
            max_width: 1920,
            max_height: 1080,
            bit_rate: 4_000_000,
            max_bit_rate: 5_000_000,
            preset: "veryfast",
        },
        CompressionProfile {
            max_width: 1280,
            max_height: 720,
            bit_rate: 2_000_000,
            max_bit_rate: 2_500_000,
            preset: "veryfast",
        },
    ];

    let mut last_error = None;
    for profile in profiles {
        let _ = std::fs::remove_file(&tmp_output);
        match transcode_h264(input_path, &tmp_output, profile) {
            Ok(()) => {
                let compressed_size = std::fs::metadata(&tmp_output)
                    .with_context(|| {
                        format!("Не удалось прочитать размер {}", tmp_output.display())
                    })?
                    .len();
                if compressed_size <= max_bytes {
                    std::fs::rename(&tmp_output, output_path).with_context(|| {
                        format!(
                            "Не удалось опубликовать сжатое видео {}",
                            output_path.display()
                        )
                    })?;
                    return Ok(());
                }

                last_error = Some(anyhow!(
                    "Сжатое видео {} все еще больше лимита {}",
                    compressed_size,
                    max_bytes
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }

    let _ = std::fs::remove_file(&tmp_output);
    Err(last_error.unwrap_or_else(|| anyhow!("Не удалось сжать видео")))
}

#[derive(Clone, Copy)]
struct CompressionProfile {
    max_width: u32,
    max_height: u32,
    bit_rate: usize,
    max_bit_rate: usize,
    preset: &'static str,
}

fn transcode_h264(
    input_path: &Path,
    output_path: &Path,
    profile: CompressionProfile,
) -> Result<()> {
    let input_path_str = input_path.to_string_lossy().to_string();
    let output_path_str = output_path.to_string_lossy().to_string();
    let mut input_ctx = format::input(&input_path_str)
        .with_context(|| format!("Не удалось открыть видео {}", input_path.display()))?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В файле нет видеопотока"))?;
    let input_stream_index = input_stream.index();
    let input_time_base = input_stream.time_base();

    let context_decoder = codec::context::Context::from_parameters(input_stream.parameters())
        .context("Не удалось создать decoder context")?;
    let mut decoder = context_decoder
        .decoder()
        .video()
        .context("Не удалось открыть video decoder")?;

    let (target_width, target_height) = scaled_dimensions(
        decoder.width(),
        decoder.height(),
        profile.max_width,
        profile.max_height,
    );
    let mut output_ctx =
        format::output(&output_path_str).context("Не удалось открыть compressed MP4 output")?;
    let global_header = output_ctx
        .format()
        .flags()
        .contains(format::Flags::GLOBAL_HEADER);

    let codec = encoder::find(codec::Id::H264).ok_or_else(|| anyhow!("H264 encoder not found"))?;
    let output_stream_index = {
        let mut output_stream = output_ctx
            .add_stream(codec)
            .context("Не удалось создать compressed output stream")?;
        let index = output_stream.index();
        let mut encoder = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .context("Не удалось создать H264 encoder")?;
        encoder.set_width(target_width);
        encoder.set_height(target_height);
        encoder.set_format(format::Pixel::YUV420P);
        encoder.set_time_base(input_time_base);
        encoder.set_frame_rate(decoder.frame_rate());
        encoder.set_aspect_ratio(Rational::new(1, 1));
        encoder.set_bit_rate(profile.bit_rate);
        encoder.set_max_bit_rate(profile.max_bit_rate);
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let mut options = Dictionary::new();
        let bit_rate = profile.bit_rate.to_string();
        let max_rate = profile.max_bit_rate.to_string();
        let buffer_size = (profile.max_bit_rate * 2).to_string();
        options.set("b", &bit_rate);
        options.set("maxrate", &max_rate);
        options.set("bufsize", &buffer_size);
        options.set("preset", profile.preset);
        let encoder = encoder
            .open_with(options)
            .context("Не удалось открыть H264 encoder")?;
        output_stream.set_parameters(&encoder);
        (index, encoder)
    };
    let (output_stream_index, mut encoder) = output_stream_index;

    let mut header_options = Dictionary::new();
    header_options.set("movflags", "+faststart");
    output_ctx
        .write_header_with(header_options)
        .context("Не удалось записать compressed MP4 header")?;
    let output_time_base = output_ctx
        .stream(output_stream_index)
        .ok_or_else(|| anyhow!("Compressed output stream missing"))?
        .time_base();

    let mut scaler = software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        format::Pixel::YUV420P,
        target_width,
        target_height,
        software::scaling::flag::Flags::BILINEAR,
    )
    .context("Не удалось создать scaler для сжатия")?;

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != input_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Не удалось отправить packet в decoder")?;
        encode_available_frames(
            &mut decoder,
            &mut scaler,
            &mut encoder,
            &mut output_ctx,
            output_stream_index,
            input_time_base,
            output_time_base,
        )?;
    }

    decoder.send_eof().ok();
    encode_available_frames(
        &mut decoder,
        &mut scaler,
        &mut encoder,
        &mut output_ctx,
        output_stream_index,
        input_time_base,
        output_time_base,
    )?;
    encoder
        .send_eof()
        .context("Не удалось flush H264 encoder")?;
    write_available_packets(
        &mut encoder,
        &mut output_ctx,
        output_stream_index,
        input_time_base,
        output_time_base,
    )?;

    output_ctx
        .write_trailer()
        .context("Не удалось записать compressed MP4 trailer")?;

    Ok(())
}

fn transcode_camera_clip(
    camera: &Camera,
    seconds: u32,
    output_path: &Path,
    profile: CompressionProfile,
) -> Result<()> {
    log::info!(
        "Camera transcode fallback started: camera={} {}, duration={}s, output={}",
        camera.id,
        camera.name,
        seconds,
        output_path.display()
    );
    let mut input_ctx = open_camera_input(&camera.stream_url)?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("В потоке камеры нет видео"))?;
    let input_stream_index = input_stream.index();
    let input_time_base = input_stream.time_base();

    let context_decoder = codec::context::Context::from_parameters(input_stream.parameters())
        .context("Не удалось создать decoder context")?;
    let mut decoder = context_decoder
        .decoder()
        .video()
        .context("Не удалось открыть video decoder")?;

    let (target_width, target_height) = scaled_dimensions(
        decoder.width(),
        decoder.height(),
        profile.max_width,
        profile.max_height,
    );
    let output_path_str = output_path.to_string_lossy().to_string();
    let mut output_ctx =
        format::output(&output_path_str).context("Не удалось открыть fallback MP4 output")?;
    let global_header = output_ctx
        .format()
        .flags()
        .contains(format::Flags::GLOBAL_HEADER);

    let codec = encoder::find(codec::Id::H264).ok_or_else(|| anyhow!("H264 encoder not found"))?;
    let output_stream_index = {
        let mut output_stream = output_ctx
            .add_stream(codec)
            .context("Не удалось создать fallback output stream")?;
        let index = output_stream.index();
        let mut encoder = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .context("Не удалось создать fallback H264 encoder")?;
        encoder.set_width(target_width);
        encoder.set_height(target_height);
        encoder.set_format(format::Pixel::YUV420P);
        encoder.set_time_base(input_time_base);
        encoder.set_frame_rate(decoder.frame_rate());
        encoder.set_aspect_ratio(Rational::new(1, 1));
        encoder.set_bit_rate(profile.bit_rate);
        encoder.set_max_bit_rate(profile.max_bit_rate);
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let mut options = Dictionary::new();
        let bit_rate = profile.bit_rate.to_string();
        let max_rate = profile.max_bit_rate.to_string();
        let buffer_size = (profile.max_bit_rate * 2).to_string();
        options.set("b", &bit_rate);
        options.set("maxrate", &max_rate);
        options.set("bufsize", &buffer_size);
        options.set("preset", profile.preset);
        let encoder = encoder
            .open_with(options)
            .context("Не удалось открыть fallback H264 encoder")?;
        output_stream.set_parameters(&encoder);
        (index, encoder)
    };
    let (output_stream_index, mut encoder) = output_stream_index;

    let mut header_options = Dictionary::new();
    header_options.set("movflags", "+faststart");
    output_ctx
        .write_header_with(header_options)
        .context("Не удалось записать fallback MP4 header")?;
    let output_time_base = output_ctx
        .stream(output_stream_index)
        .ok_or_else(|| anyhow!("Fallback output stream missing"))?
        .time_base();

    let mut scaler = software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        format::Pixel::YUV420P,
        target_width,
        target_height,
        software::scaling::flag::Flags::BILINEAR,
    )
    .context("Не удалось создать scaler для fallback записи")?;

    let mut capture_started_at = None;
    let mut encoded_frames = 0usize;
    for (stream, packet) in input_ctx.packets() {
        if stream.index() != input_stream_index {
            continue;
        }

        if capture_started_at.is_some_and(|started_at: Instant| {
            started_at.elapsed() >= Duration::from_secs(seconds as u64)
        }) {
            break;
        }

        decoder
            .send_packet(&packet)
            .context("Не удалось отправить packet в fallback decoder")?;
        let frames = encode_available_frames(
            &mut decoder,
            &mut scaler,
            &mut encoder,
            &mut output_ctx,
            output_stream_index,
            input_time_base,
            output_time_base,
        )?;
        if frames > 0 {
            encoded_frames += frames;
            capture_started_at.get_or_insert_with(Instant::now);
        }
    }

    if encoded_frames == 0 {
        return Err(anyhow!(
            "Не удалось декодировать video frames из камеры для fallback записи"
        ));
    }

    decoder.send_eof().ok();
    encode_available_frames(
        &mut decoder,
        &mut scaler,
        &mut encoder,
        &mut output_ctx,
        output_stream_index,
        input_time_base,
        output_time_base,
    )?;
    encoder
        .send_eof()
        .context("Не удалось flush fallback H264 encoder")?;
    write_available_packets(
        &mut encoder,
        &mut output_ctx,
        output_stream_index,
        input_time_base,
        output_time_base,
    )?;

    output_ctx
        .write_trailer()
        .context("Не удалось записать fallback MP4 trailer")?;
    log::info!(
        "Camera transcode fallback finished: camera={} {}, frames={}, output={}",
        camera.id,
        camera.name,
        encoded_frames,
        output_path.display()
    );

    Ok(())
}

fn encode_available_frames(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut software::scaling::context::Context,
    encoder: &mut encoder::Video,
    output_ctx: &mut format::context::Output,
    output_stream_index: usize,
    input_time_base: Rational,
    output_time_base: Rational,
) -> Result<usize> {
    let mut decoded = frame::Video::empty();
    let mut encoded_frames = 0usize;
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut scaled = frame::Video::empty();
        scaler
            .run(&decoded, &mut scaled)
            .context("Не удалось масштабировать кадр")?;
        scaled.set_pts(decoded.timestamp());
        scaled.set_kind(picture::Type::None);
        encoder
            .send_frame(&scaled)
            .context("Не удалось отправить кадр в H264 encoder")?;
        write_available_packets(
            encoder,
            output_ctx,
            output_stream_index,
            input_time_base,
            output_time_base,
        )?;
        encoded_frames += 1;
    }

    Ok(encoded_frames)
}

fn write_available_packets(
    encoder: &mut encoder::Video,
    output_ctx: &mut format::context::Output,
    output_stream_index: usize,
    input_time_base: Rational,
    output_time_base: Rational,
) -> Result<()> {
    let mut encoded = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut encoded).is_ok() {
        encoded.set_stream(output_stream_index);
        encoded.rescale_ts(input_time_base, output_time_base);
        encoded.write_interleaved(output_ctx)?;
    }

    Ok(())
}

fn scaled_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let (max_width, max_height) = if height > width && max_width > max_height {
        (max_height, max_width)
    } else {
        (max_width, max_height)
    };

    if width <= max_width && height <= max_height {
        return (make_even(width), make_even(height));
    }

    let scale = (max_width as f64 / width as f64).min(max_height as f64 / height as f64);
    let scaled_width = make_even_nearest((width as f64 * scale).round() as u32);
    let scaled_height = make_even_nearest((height as f64 * scale).round() as u32);
    (scaled_width.max(2), scaled_height.max(2))
}

fn make_even(value: u32) -> u32 {
    make_even_nearest(value)
}

fn make_even_nearest(value: u32) -> u32 {
    if value.is_multiple_of(2) {
        value
    } else {
        value.saturating_add(1).max(2)
    }
}

fn remux_camera_clip(camera: &Camera, seconds: u32, output_path: &Path) -> Result<()> {
    log::info!(
        "Camera remux started: camera={} {}, duration={}s, output={}",
        camera.id,
        camera.name,
        seconds,
        output_path.display()
    );
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
    let keyframe_wait_started_at = Instant::now();
    let mut record_started_at = None;

    for (stream, mut packet) in input_ctx.packets() {
        if stream.index() != input_stream_index {
            continue;
        }

        if !started_on_keyframe {
            if keyframe_wait_started_at.elapsed() >= Duration::from_secs(REMUX_KEYFRAME_WAIT_S) {
                break;
            }

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
        return Err(anyhow!(
            "Не удалось получить video packets из камеры: keyframe не пришел за {}с",
            REMUX_KEYFRAME_WAIT_S
        ));
    }

    output_ctx
        .write_trailer()
        .context("Не удалось записать MP4 trailer")?;
    log::info!(
        "Camera remux finished: camera={} {}, packets={}, output={}",
        camera.id,
        camera.name,
        wrote_packets,
        output_path
    );

    Ok(())
}

fn safe_stream_label(stream_url: &str) -> String {
    if let Some((scheme, rest)) = stream_url.split_once("://") {
        let host = rest
            .rsplit_once('@')
            .map(|(_, value)| value)
            .unwrap_or(rest)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if host.is_empty() {
            return format!("{}://<hidden>", scheme);
        }

        return format!("{}://{}", scheme, host);
    }

    stream_url
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("<hidden>")
        .to_string()
}

pub(crate) fn sanitize_camera_error(error: &anyhow::Error) -> String {
    sanitize_camera_error_text(&format!("{:#}", error))
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn sanitize_camera_error_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut offset = 0;

    while let Some(relative_start) = find_next_media_url(&text[offset..]) {
        let start = offset + relative_start;
        result.push_str(&text[offset..start]);

        let url_len = media_url_len(&text[start..]);
        let end = start + url_len;
        result.push_str(&safe_stream_label(&text[start..end]));
        offset = end;
    }

    result.push_str(&text[offset..]);
    result
}

fn find_next_media_url(text: &str) -> Option<usize> {
    ["rtsp://", "rtsps://", "http://", "https://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min()
}

fn media_url_len(text: &str) -> usize {
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']') {
            return idx;
        }

        if ch == ':' && chars.peek().is_some_and(|(_, next)| next.is_whitespace()) {
            return idx;
        }
    }

    text.len()
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

    format::input_with_dictionary(stream_url, options).map_err(|error| {
        anyhow!(
            "Не удалось открыть видеопоток {}: {}",
            safe_stream_label(stream_url),
            sanitize_camera_error_text(&error.to_string())
        )
    })
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

fn tmp_output_path(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let extension = output_path
        .extension()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();

    if extension.is_empty() {
        output_path.with_file_name(format!(".{}.tmp", stem))
    } else {
        output_path.with_file_name(format!(".{}.tmp.{}", stem, extension))
    }
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

    #[test]
    fn scaled_dimensions_preserve_aspect_and_even_values() {
        assert_eq!(scaled_dimensions(1920, 1080, 1920, 1080), (1920, 1080));
        assert_eq!(scaled_dimensions(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(scaled_dimensions(1080, 1920, 1280, 720), (720, 1280));
    }

    #[test]
    fn scaled_dimensions_keep_small_even_frame() {
        assert_eq!(scaled_dimensions(640, 360, 1280, 720), (640, 360));
    }

    #[test]
    fn tmp_output_path_keeps_media_extension_for_ffmpeg_muxer_detection() {
        let path = Path::new("data/recordings/camera_3_session_1_segment_1.telegram.mp4");

        assert_eq!(
            tmp_output_path(path),
            PathBuf::from("data/recordings/.camera_3_session_1_segment_1.telegram.tmp.mp4")
        );
    }

    #[test]
    fn safe_stream_label_hides_credentials() {
        assert_eq!(
            safe_stream_label("rtsp://user:secret@192.168.1.20:554/live?token=abc"),
            "rtsp://192.168.1.20:554"
        );
        assert_eq!(
            safe_stream_label("http://camera.local/snapshot?token=abc"),
            "http://camera.local"
        );
        assert_eq!(
            safe_stream_label("http://camera.local?token=abc"),
            "http://camera.local"
        );
    }

    #[test]
    fn camera_error_text_redacts_media_urls() {
        let text = sanitize_camera_error_text(
            "failed rtsp://user:secret@192.168.1.20:554/live?token=abc: denied; snapshot https://camera.local/snapshot?token=abc",
        );

        assert!(text.contains("rtsp://192.168.1.20:554"));
        assert!(text.contains("https://camera.local"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("token=abc"));
        assert!(!text.contains("/snapshot"));
    }
}
