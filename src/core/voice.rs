use anyhow::{anyhow, Context, Result};
use ffmpeg::{codec, format, frame, media, software, ChannelLayout};
use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

static FFMPEG_INIT: Once = Once::new();

pub async fn decode_audio_file_to_pcm_mono(path: PathBuf, sample_rate: u32) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || decode_audio_file_to_pcm_mono_blocking(&path, sample_rate))
        .await
        .context("Не удалось запустить декодирование аудио")?
}

fn init_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg init failed");
        ffmpeg::log::set_level(ffmpeg::log::Level::Warning);
    });
}

fn decode_audio_file_to_pcm_mono_blocking(path: &Path, sample_rate: u32) -> Result<Vec<u8>> {
    init_ffmpeg();

    let mut input_ctx = format::input(path).with_context(|| "Не удалось открыть voice-файл")?;
    let input_stream = input_ctx
        .streams()
        .best(media::Type::Audio)
        .context("В voice-файле нет аудио дорожки")?;
    let stream_index = input_stream.index();

    let context_decoder = codec::context::Context::from_parameters(input_stream.parameters())
        .context("Не удалось создать audio decoder context")?;
    let mut decoder = context_decoder
        .decoder()
        .audio()
        .context("Не удалось открыть audio decoder")?;

    let in_layout = if decoder.channel_layout().is_empty() {
        ChannelLayout::default(i32::from(decoder.channels()))
    } else {
        decoder.channel_layout()
    };
    let out_layout = ChannelLayout::MONO;
    let out_format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
    let mut resampler = software::resampling::Context::get(
        decoder.format(),
        in_layout,
        decoder.rate(),
        out_format,
        out_layout,
        sample_rate,
    )
    .context("Не удалось создать audio resampler")?;

    let mut pcm = Vec::new();
    let mut decoded = frame::Audio::empty();
    let mut converted = frame::Audio::empty();

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Не удалось отправить audio packet в decoder")?;
        receive_audio_frames(
            &mut decoder,
            &mut resampler,
            &mut decoded,
            &mut converted,
            &mut pcm,
        )?;
    }

    decoder.send_eof().ok();
    receive_audio_frames(
        &mut decoder,
        &mut resampler,
        &mut decoded,
        &mut converted,
        &mut pcm,
    )?;
    loop {
        match resampler.flush(&mut converted) {
            Ok(Some(_)) => append_i16_audio(&converted, &mut pcm)?,
            Ok(None) => break,
            Err(_) => break,
        }
    }

    if pcm.is_empty() {
        return Err(anyhow!("STT получил пустой PCM после декодирования"));
    }

    Ok(pcm)
}

fn receive_audio_frames(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut software::resampling::Context,
    decoded: &mut frame::Audio,
    converted: &mut frame::Audio,
    pcm: &mut Vec<u8>,
) -> Result<()> {
    while decoder.receive_frame(decoded).is_ok() {
        resampler
            .run(decoded, converted)
            .context("Не удалось конвертировать аудио в PCM")?;
        append_i16_audio(converted, pcm)?;
    }

    Ok(())
}

fn append_i16_audio(frame: &frame::Audio, pcm: &mut Vec<u8>) -> Result<()> {
    if frame.samples() == 0 {
        return Ok(());
    }

    let samples = frame.plane::<i16>(0);
    pcm.reserve(samples.len() * 2);
    for sample in samples {
        pcm.extend_from_slice(&sample.to_le_bytes());
    }

    Ok(())
}

pub fn estimated_pcm_duration(pcm_len: usize, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        return Duration::ZERO;
    }

    let samples = pcm_len / 2;
    Duration::from_secs_f64(samples as f64 / sample_rate as f64)
}
