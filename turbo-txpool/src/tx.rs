// Copyright 2021 ConsenSys
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Transactions as understood by the transaction pool.

use crate::error::{Error, RlpResultExt};

use ethereum_types::{Address, H256, U256};

pub use k256::ecdsa::Error as EcdsaError;
use k256::ecdsa::{self, recoverable};
use k256::EncodedPoint;

use std::convert::{TryFrom, TryInto};

use tiny_keccak::{Hasher, Keccak};

fn keccak(input: &[u8]) -> [u8; 32] {
    let mut keccak = Keccak::v256();
    keccak.update(input);

    let mut output = [0u8; 32];
    keccak.finalize(&mut output);
    output
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedTx {
    hash: H256,
    from: Address,
    tx: Tx,
}

impl VerifiedTx {
    pub fn new(tx: Tx) -> Result<Self, EcdsaError> {
        let v = 1 - (tx.v % 2);
        let mut r = [0u8; 32];
        tx.r.to_big_endian(&mut r);
        let mut s = [0u8; 32];
        tx.s.to_big_endian(&mut s);

        let sig = recoverable::Signature::new(
            &ecdsa::Signature::from_scalars(r, s)?,
            recoverable::Id::new(v.try_into().unwrap())?,
        )?;

        let mut stream = rlp::RlpStream::new();
        tx.signature_encode(&mut stream);
        let vkey = sig.recover_verify_key(stream.as_raw())?;

        let point = EncodedPoint::from(&vkey).decompress();

        let pubkey = match point {
            Some(k) => k.to_bytes(),
            None => return Err(EcdsaError::new()),
        };

        let from = Address::from_slice(&keccak(&pubkey[1..])[12..]);

        stream.clear();
        tx.encode(&mut stream);
        let hash = H256::from(keccak(stream.as_raw()));

        Ok(Self { hash, from, tx })
    }

    pub fn gas_limit(&self) -> u64 {
        self.tx.gas_limit
    }

    pub fn gas_price(&self) -> &U256 {
        &self.tx.gas_price
    }

    pub fn hash(&self) -> &H256 {
        &self.hash
    }

    pub fn tx(&self) -> &Tx {
        &self.tx
    }

    pub fn from(&self) -> &Address {
        &self.from
    }

    pub fn nonce(&self) -> u64 {
        self.tx.nonce
    }

    pub fn value(&self) -> &U256 {
        &self.tx.value
    }

    // TODO: Add getters as needed.
}

impl TryFrom<Tx> for VerifiedTx {
    type Error = EcdsaError;

    fn try_from(tx: Tx) -> Result<Self, Self::Error> {
        VerifiedTx::new(tx)
    }
}

impl Eq for VerifiedTx {}

impl PartialEq for VerifiedTx {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

/// An Ethereum transaction.
#[derive(Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[non_exhaustive]
pub struct Tx {
    /// The unique index of the transaction, used for replay protection.
    pub nonce: u64,

    /// The destination address of the transaction, or `None` when creating a
    /// contract.
    pub to: Option<Address>,

    /// The value, in Wei, sent along with this transaction.
    pub value: U256,

    /// The amount of Wei to transfer to the miner, per unit of gas consumed.
    pub gas_price: U256,

    /// The maximum units of gas this transaction is allowed to consume.
    pub gas_limit: u64,

    /// The calldata (or contract initialization code) sent with the
    /// transaction.
    pub input: Vec<u8>,

    /// The `v` component (or recovery id) of the transaction signature.
    pub v: u64,

    /// The `r` component of the transaction signature.
    pub r: U256,

    /// The `s` component of the transaction signature.
    pub s: U256,
}

impl Tx {
    fn signature_encode(&self, stream: &mut rlp::RlpStream) {
        if self.v >= 35 {
            self.signature_encode_9(stream);
        } else {
            self.signature_encode_6(stream);
        }
    }

    fn signature_encode_6(&self, stream: &mut rlp::RlpStream) {
        stream.begin_list(6);
        stream.append(&U256::from(self.nonce));
        stream.append(&self.gas_price);
        stream.append(&U256::from(self.gas_limit));
        match self.to {
            None => {
                stream.append_empty_data();
            }
            Some(to) => {
                stream.append(&to);
            }
        }
        stream.append(&self.value);
        stream.append(&self.input);
    }

    fn signature_encode_9(&self, stream: &mut rlp::RlpStream) {
        let chainid = (self.v - 35) / 2;
        stream.begin_list(9);
        stream.append(&U256::from(self.nonce));
        stream.append(&self.gas_price);
        stream.append(&U256::from(self.gas_limit));
        match self.to {
            None => {
                stream.append_empty_data();
            }
            Some(to) => {
                stream.append(&to);
            }
        }
        stream.append(&self.value);
        stream.append(&self.input);
        stream.append(&chainid);
        stream.append(&0u8);
        stream.append(&0u8);
    }

    pub(crate) fn encode(&self, stream: &mut rlp::RlpStream) {
        stream.begin_list(9);
        stream.append(&U256::from(self.nonce));
        stream.append(&self.gas_price);
        stream.append(&U256::from(self.gas_limit));
        match self.to {
            None => {
                stream.append_empty_data();
            }
            Some(to) => {
                stream.append(&to);
            }
        }
        stream.append(&self.value);
        stream.append(&self.input);
        stream.append(&self.v);
        stream.append(&self.r);
        stream.append(&self.s);
    }

    pub(crate) fn decode(stream: &rlp::Rlp) -> Result<Self, Error> {
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

#[cfg(test)]
mod tests {
    use hex_literal::hex;

    use super::*;

    #[test]
    fn encode_deploy() {
        let input: &[_] = &hex!(
            "
            6080604052336000806101000a81548173ffffffffffffffffffffffffffffffff
            ffffffff021916908373ffffffffffffffffffffffffffffffffffffffff160217
            90555034801561005057600080fd5b50610207806100606000396000f3fe608060
            405234801561001057600080fd5b50600436106100415760003560e01c8063445d
            f0ac146100465780638da5cb5b14610064578063fdacd57614610098575b600080
            fd5b61004e6100c6565b6040518082815260200191505060405180910390f35b61
            006c6100cc565b604051808273ffffffffffffffffffffffffffffffffffffffff
            16815260200191505060405180910390f35b6100c4600480360360208110156100
            ae57600080fd5b81019080803590602001909291905050506100f0565b005b6001
            5481565b60008054906101000a900473ffffffffffffffffffffffffffffffffff
            ffffff1681565b60008054906101000a900473ffffffffffffffffffffffffffff
            ffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffff
            ffffffffffffffffffffffffffffffffffff1614610194576040517f08c379a000
            000000000000000000000000000000000000000000000000000000815260040180
            806020018281038252603381526020018061019f60339139604001915050604051
            80910390fd5b806001819055505056fe546869732066756e6374696f6e20697320
            7265737472696374656420746f2074686520636f6e74726163742773206f776e65
            72a26469706673582212208400e0286af371ce920275ae3fb332725ab7611fc4e1
            e0bef24019ef724b0e4464736f6c634300060c0033
            "
        );

        let r = U256::from(hex!(
            "
            241881bc2b3c3fd940aaec9de425409d63f1c2101aae76dc1ff6747d8998f3e6
            "
        ));

        let s = U256::from(hex!(
            "
            5fed35536db8a9c90c1d1e3a9d8b6669d4d3b0722228e38e6f6cc571272ed0ab
            "
        ));

        let tx = Tx {
            value: 0.into(),
            to: None,
            gas_limit: 6_721_975,
            gas_price: 20_000_000_000u64.into(),
            input: input.into(),
            nonce: 82,
            v: 28,
            r,
            s,
        };

        let mut stream = rlp::RlpStream::new();
        tx.encode(&mut stream);
        let actual = stream.as_raw();

        let expected: &[_] = &hex!(
            "
            f902ba528504a817c800836691b78080b902676080604052336000806101000a81
            548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffff
            ffffffffffffffffffffffffffffffff16021790555034801561005057600080fd
            5b50610207806100606000396000f3fe608060405234801561001057600080fd5b
            50600436106100415760003560e01c8063445df0ac146100465780638da5cb5b14
            610064578063fdacd57614610098575b600080fd5b61004e6100c6565b60405180
            82815260200191505060405180910390f35b61006c6100cc565b604051808273ff
            ffffffffffffffffffffffffffffffffffffff1681526020019150506040518091
            0390f35b6100c4600480360360208110156100ae57600080fd5b81019080803590
            602001909291905050506100f0565b005b60015481565b60008054906101000a90
            0473ffffffffffffffffffffffffffffffffffffffff1681565b60008054906101
            000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffff
            ffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffff
            ffffffff1614610194576040517f08c379a0000000000000000000000000000000
            000000000000000000000000008152600401808060200182810382526033815260
            20018061019f6033913960400191505060405180910390fd5b8060018190555050
            56fe546869732066756e6374696f6e206973207265737472696374656420746f20
            74686520636f6e74726163742773206f776e6572a26469706673582212208400e0
            286af371ce920275ae3fb332725ab7611fc4e1e0bef24019ef724b0e4464736f6c
            634300060c00331ca0241881bc2b3c3fd940aaec9de425409d63f1c2101aae76dc
            1ff6747d8998f3e6a05fed35536db8a9c90c1d1e3a9d8b6669d4d3b0722228e38e
            6f6cc571272ed0ab
            "
        );

        assert_eq!(expected, actual);
    }

    #[test]
    fn decode() -> Result<(), Error> {
        let txbytes = hex!(
            "
            f8658001887fffffffffffffff94095e7baea6a6c7c4c2dfeb977efac326af552d
            8780801ba048b55bfa915ac795c431978d8a6a992b628d557da5ff759b307d495a
            36649353a0efffd310ac743f371de3b9f7f9cb56c0b28ad43601b4ab949f53faa0
            7bd2c804
            "
        );

        let tx = Tx::decode(&rlp::Rlp::new(&txbytes))?;
        assert_eq!(tx.input, &[]);
        assert_eq!(tx.gas_limit, 0x7fffffffffffffff);
        assert_eq!(tx.gas_price, 1.into());
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.value, 0.into());
        assert_eq!(tx.v, 27);
        assert_eq!(
            tx.to,
            Some(hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87").into())
        );
        assert_eq!(
            tx.r,
            hex!(
                "
                48b55bfa915ac795c431978d8a6a992b628d557da5ff759b307d495a36649353
                "
            )
            .into()
        );

        assert_eq!(
            tx.s,
            hex!(
                "
                efffd310ac743f371de3b9f7f9cb56c0b28ad43601b4ab949f53faa07bd2c804
                "
            )
            .into()
        );

        let vtx = VerifiedTx::new(tx).unwrap();
        let hash = H256::from(hex!(
            "209466e8f6b969c2075bf546938265a13eee23caf7d59e4d7c4e54c26eb482ec"
        ));
        assert_eq!(vtx.hash, hash);

        Ok(())
    }

    #[test]
    fn decode_1000000() -> Result<(), Error> {
        let txbytes = hex!(
            "
            f86e158512bfb19e608301f8dc94c083e9947cf02b8ffc7d3090ae9aea72df98fd
            4789056bc75e2d63100000801ca0a254fe085f721c2abe00a2cd244110bfc0df5f
            4f25461c85d8ab75ebac11eb10a030b7835ba481955b20193a703ebc5fdffeab08
            1d63117199040cdf5a91c68765
            "
        );

        let tx = Tx::decode(&rlp::Rlp::new(&txbytes))?;
        assert_eq!(tx.input, &[]);
        assert_eq!(tx.gas_limit, 129244);
        assert_eq!(tx.gas_price, 80525500000u64.into());
        assert_eq!(tx.nonce, 21);
        assert_eq!(tx.value, 100000000000000000000u128.into());
        assert_eq!(tx.v, 0x1c);
        assert_eq!(
            tx.to,
            Some(hex!("c083e9947cf02b8ffc7d3090ae9aea72df98fd47").into())
        );
        assert_eq!(
            tx.r,
            hex!(
                "
                a254fe085f721c2abe00a2cd244110bfc0df5f4f25461c85d8ab75ebac11eb10
                "
            )
            .into()
        );

        assert_eq!(
            tx.s,
            hex!(
                "
                30b7835ba481955b20193a703ebc5fdffeab081d63117199040cdf5a91c68765
                "
            )
            .into()
        );

        let vtx = VerifiedTx::new(tx).unwrap();
        let hash = H256::from(hex!(
            "ea1093d492a1dcb1bef708f771a99a96ff05dcab81ca76c31940300177fcf49f"
        ));
        assert_eq!(vtx.hash, hash);

        let from =
            Address::from(hex!("39fa8c5f2793459d6622857e7d9fbb4bd91766d3"));
        assert_eq!(vtx.from, from);

        Ok(())
    }
}
