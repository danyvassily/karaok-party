use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Audio I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Audio decode error: {0}")]
    Decode(String),

    #[error("Resampling error: {0}")]
    Resample(String),

    #[error("CoreAudio device error: {0}")]
    Device(String),

    #[error("WAV encoding error: {0}")]
    Wav(#[from] hound::Error),

    #[error("Recording already active or not active: {0}")]
    Recorder(String),
}

pub type AudioResult<T> = Result<T, AudioError>;
