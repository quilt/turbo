use crate::error::{Error, RlpResultExt};

use ethereum_types::{Address, H256, U256};

use tiny_keccak::{Hasher, Keccak};

fn keccak(input: &[u8]) -> H256 {
    let mut keccak = Keccak::v256();
    keccak.update(input);

    let mut output = [0u8; 32];
    keccak.finalize(&mut output);
    H256::from(output)
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Tx {
    pub hash: H256,
    pub nonce: u64,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_price: U256,
    pub gas_limit: u64,
    pub input: Vec<u8>,
    pub v: u64,
    pub r: U256,
    pub s: U256,
}

impl Tx {
    pub fn decode(stream: &rlp::Rlp) -> Result<Self, Error> {
        let hash = keccak(stream.as_raw());

        let to = {
            let field = stream.at(3).context_field("to")?;
            if field.is_empty() {
                if field.is_data() {
                    None
                } else {
                    return Err(Error::RlpDecode {
                        source: rlp::DecoderError::RlpExpectedToBeData,
                        field: Some("to"),
                    });
                }
            } else {
                Some(field.as_val().context_field("to")?)
            }
        };

        let v = stream.val_at(6).context_field("v")?;
        let r = stream.val_at(7).context_field("r")?;
        let s = stream.val_at(8).context_field("s")?;

        let nonce = match stream.val_at::<U256>(0).context_field("nonce")? {
            x if x > u64::max_value().into() => {
                return Err(Error::IntegerOverflow);
            }
            x => x.as_u64(),
        };

        let gas_limit =
            match stream.val_at::<U256>(2).context_field("gas_limit")? {
                x if x > u64::max_value().into() => {
                    return Err(Error::IntegerOverflow);
                }
                x => x.as_u64(),
            };

        Ok(Self {
            hash,
            nonce,
            gas_price: stream.val_at(1).context_field("gas_price")?,
            gas_limit,
            to,
            value: stream.val_at(4).context_field("value")?,
            input: stream.val_at(5).context_field("input")?,
            v,
            r,
            s,
        })
    }
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub struct Priced {
    pub gas_price: U256,
    pub key: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_decode() -> Result<(), Error> {
        let txbytes = [
            0xf8, 0x65, 0x80, 0x01, 0x88, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0x94, 0x09, 0x5e, 0x7b, 0xae, 0xa6, 0xa6, 0xc7, 0xc4,
            0xc2, 0xdf, 0xeb, 0x97, 0x7e, 0xfa, 0xc3, 0x26, 0xaf, 0x55, 0x2d,
            0x87, 0x80, 0x80, 0x1b, 0xa0, 0x48, 0xb5, 0x5b, 0xfa, 0x91, 0x5a,
            0xc7, 0x95, 0xc4, 0x31, 0x97, 0x8d, 0x8a, 0x6a, 0x99, 0x2b, 0x62,
            0x8d, 0x55, 0x7d, 0xa5, 0xff, 0x75, 0x9b, 0x30, 0x7d, 0x49, 0x5a,
            0x36, 0x64, 0x93, 0x53, 0xa0, 0xef, 0xff, 0xd3, 0x10, 0xac, 0x74,
            0x3f, 0x37, 0x1d, 0xe3, 0xb9, 0xf7, 0xf9, 0xcb, 0x56, 0xc0, 0xb2,
            0x8a, 0xd4, 0x36, 0x01, 0xb4, 0xab, 0x94, 0x9f, 0x53, 0xfa, 0xa0,
            0x7b, 0xd2, 0xc8, 0x04,
        ];

        let hash = H256::from([
            0x20, 0x94, 0x66, 0xe8, 0xf6, 0xb9, 0x69, 0xc2, 0x07, 0x5b, 0xf5,
            0x46, 0x93, 0x82, 0x65, 0xa1, 0x3e, 0xee, 0x23, 0xca, 0xf7, 0xd5,
            0x9e, 0x4d, 0x7c, 0x4e, 0x54, 0xc2, 0x6e, 0xb4, 0x82, 0xec,
        ]);

        let tx = Tx::decode(&rlp::Rlp::new(&txbytes))?;
        assert_eq!(tx.hash, hash);
        assert_eq!(tx.input, &[]);
        assert_eq!(tx.gas_limit, 0x7fffffffffffffff);
        assert_eq!(tx.gas_price, 1.into());
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.value, 0.into());
        assert_eq!(tx.v, 27);
        assert_eq!(
            tx.to,
            Some(
                [
                    0x09, 0x5e, 0x7b, 0xae, 0xa6, 0xa6, 0xc7, 0xc4, 0xc2, 0xdf,
                    0xeb, 0x97, 0x7e, 0xfa, 0xc3, 0x26, 0xaf, 0x55, 0x2d, 0x87
                ]
                .into()
            )
        );
        assert_eq!(
            tx.r,
            [
                0x48, 0xb5, 0x5b, 0xfa, 0x91, 0x5a, 0xc7, 0x95, 0xc4, 0x31,
                0x97, 0x8d, 0x8a, 0x6a, 0x99, 0x2b, 0x62, 0x8d, 0x55, 0x7d,
                0xa5, 0xff, 0x75, 0x9b, 0x30, 0x7d, 0x49, 0x5a, 0x36, 0x64,
                0x93, 0x53
            ]
            .into()
        );

        assert_eq!(
            tx.s,
            [
                0xef, 0xff, 0xd3, 0x10, 0xac, 0x74, 0x3f, 0x37, 0x1d, 0xe3,
                0xb9, 0xf7, 0xf9, 0xcb, 0x56, 0xc0, 0xb2, 0x8a, 0xd4, 0x36,
                0x01, 0xb4, 0xab, 0x94, 0x9f, 0x53, 0xfa, 0xa0, 0x7b, 0xd2,
                0xc8, 0x04,
            ]
            .into()
        );

        Ok(())
    }
}
