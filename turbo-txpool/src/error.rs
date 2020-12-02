use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(crate)")]
pub enum Error {
    RlpDecode {
        source: rlp::DecoderError,
        field: Option<&'static str>,
    },
    IntegerOverflow,
}

pub(crate) trait RlpResultExt<T> {
    fn context_field(self, field: &'static str) -> Result<T, Error>;
}

impl<T> RlpResultExt<T> for Result<T, rlp::DecoderError> {
    fn context_field(self, field: &'static str) -> Result<T, Error> {
        self.context(RlpDecode { field })
    }
}
