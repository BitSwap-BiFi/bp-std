// Modern, minimalistic & standard-compliant cold wallet library.
//
// SPDX-License-Identifier: Apache-2.0
//
// Written in 2020-2023 by
//     Dr Maxim Orlovsky <orlovsky@lnp-bp.org>
//
// Copyright (C) 2020-2023 LNP/BP Standards Association. All rights reserved.
// Copyright (C) 2020-2023 Dr Maxim Orlovsky. All rights reserved.
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

use std::borrow::Borrow;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use amplify::{hex, Array, Bytes32, RawArray, Wrapper};
use bc::secp256k1;
use bc::secp256k1::{PublicKey, XOnlyPublicKey, SECP256K1};
use hashes::{hash160, sha512, Hash, HashEngine, Hmac, HmacEngine};

use crate::{
    base58, ComprPubkey, DerivationIndex, DerivationParseError, DerivationPath, HardenedIndex, Idx,
    NormalIndex,
};

pub const XPUB_MAINNET_MAGIC: [u8; 4] = [0x04u8, 0x88, 0xB2, 0x1E];
pub const XPUB_TESTNET_MAGIC: [u8; 4] = [0x04u8, 0x35, 0x87, 0xCF];

#[derive(Copy, Clone, Eq, PartialEq, Debug, Display, Error, From)]
#[display(doc_comments)]
pub enum XpubDecodeError {
    /// wrong length of extended pubkey data ({0}).
    WrongExtendedKeyLength(usize),

    /// provided key is not a standard BIP-32 extended pubkey
    UnknownKeyType([u8; 4]),

    /// extended pubkey contains invalid Secp256k1 pubkey data
    #[from(bc::secp256k1::Error)]
    InvalidPublicKey,
}

#[derive(Clone, Eq, PartialEq, Debug, Display, Error, From)]
pub enum XpubParseError {
    /// wrong Base58 encoding of extended pubkey data - {0}
    #[display(doc_comments)]
    #[from]
    Base58(base58::Error),

    #[display(inner)]
    #[from]
    Decode(XpubDecodeError),

    #[display(inner)]
    #[from]
    DerivationPath(DerivationParseError),

    /// invalid master key fingerprint - {0}
    #[from]
    InvalidMasterFp(hex::Error),

    /// no xpub key origin information.
    NoOrigin,

    /// xpub network and origin mismatch.
    NetworkMismatch,

    /// xpub depth and origin mismatch.
    DepthMismatch,

    /// xpub parent not matches the provided origin information.
    ParentMismatch,
}

/// BIP32 chain code used for hierarchical derivation
#[derive(Wrapper, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, From)]
#[wrapper(Deref, RangeOps)]
pub struct ChainCode(Bytes32);

impl AsRef<[u8]> for ChainCode {
    fn as_ref(&self) -> &[u8] { self.0.as_ref() }
}

impl From<[u8; 32]> for ChainCode {
    fn from(value: [u8; 32]) -> Self { Self(value.into()) }
}

impl From<ChainCode> for [u8; 32] {
    fn from(value: ChainCode) -> Self { value.0.into_inner() }
}

/// Deterministic part of the extended public key.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct XpubCore {
    /// Public key
    pub public_key: PublicKey,
    /// BIP32 chain code used for hierarchical derivation
    pub chain_code: ChainCode,
}

#[derive(Wrapper, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Default, Debug, Display, From)]
#[wrapper(RangeOps, Hex, FromStr)]
#[display(LowerHex)]
pub struct XpubFp(Array<u8, 4>);

impl AsRef<[u8]> for XpubFp {
    fn as_ref(&self) -> &[u8] { self.0.as_ref() }
}

impl From<[u8; 4]> for XpubFp {
    fn from(value: [u8; 4]) -> Self { Self(value.into()) }
}

impl From<XpubFp> for [u8; 4] {
    fn from(value: XpubFp) -> Self { value.0.into_inner() }
}

#[derive(Wrapper, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Default, Debug, Display, From)]
#[wrapper(RangeOps, Hex, FromStr)]
#[display(LowerHex)]
pub struct XpubId(Array<u8, 20>);

impl AsRef<[u8]> for XpubId {
    fn as_ref(&self) -> &[u8] { self.0.as_ref() }
}

impl From<[u8; 20]> for XpubId {
    fn from(value: [u8; 20]) -> Self { Self(value.into()) }
}

impl From<XpubId> for [u8; 20] {
    fn from(value: XpubId) -> Self { value.0.into_inner() }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct XpubMeta {
    pub depth: u8,
    pub parent_fp: XpubFp,
    pub child_number: DerivationIndex,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Xpub {
    testnet: bool,
    meta: XpubMeta,
    core: XpubCore,
}

impl Xpub {
    pub fn decode(data: impl Borrow<[u8]>) -> Result<Xpub, XpubDecodeError> {
        let data = data.borrow();

        if data.len() != 78 {
            return Err(XpubDecodeError::WrongExtendedKeyLength(data.len()));
        }

        let testnet = match &data[0..4] {
            magic if magic == XPUB_MAINNET_MAGIC => false,
            magic if magic == XPUB_TESTNET_MAGIC => true,
            unknown => {
                let mut magic = [0u8; 4];
                magic.copy_from_slice(unknown);
                return Err(XpubDecodeError::UnknownKeyType(magic));
            }
        };
        let depth = data[4];

        let mut parent_fp = [0u8; 4];
        parent_fp.copy_from_slice(&data[5..9]);

        let mut child_number = [0u8; 4];
        child_number.copy_from_slice(&data[9..13]);
        let child_number = u32::from_be_bytes(child_number);

        let mut chain_code = [0u8; 32];
        chain_code.copy_from_slice(&data[13..45]);

        let public_key = PublicKey::from_slice(&data[45..78])?;

        Ok(Xpub {
            testnet,
            meta: XpubMeta {
                depth,
                parent_fp: parent_fp.into(),
                child_number: child_number.into(),
            },
            core: XpubCore {
                public_key,
                chain_code: chain_code.into(),
            },
        })
    }

    pub fn encode(&self) -> [u8; 78] {
        let mut ret = [0; 78];
        ret[0..4].copy_from_slice(&match self.testnet {
            false => XPUB_MAINNET_MAGIC,
            true => XPUB_TESTNET_MAGIC,
        });
        ret[4] = self.meta.depth;
        ret[5..9].copy_from_slice(self.meta.parent_fp.as_ref());
        ret[9..13].copy_from_slice(&self.meta.child_number.index().to_be_bytes());
        ret[13..45].copy_from_slice(self.core.chain_code.as_ref());
        ret[45..78].copy_from_slice(&self.core.public_key.serialize());
        ret
    }

    /// Returns the HASH160 of the chaincode
    pub fn identifier(&self) -> XpubId {
        let hash = hash160::Hash::hash(&self.core.public_key.serialize());
        XpubId::from_raw_array(*hash.as_byte_array())
    }

    pub fn fingerprint(&self) -> XpubFp {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&self.identifier()[..4]);
        XpubFp::from_raw_array(bytes)
    }

    /// Constructs ECDSA public key matching internal public key representation.
    pub fn to_compr_pub(&self) -> ComprPubkey { ComprPubkey(self.core.public_key) }

    /// Constructs BIP340 public key matching internal public key representation.
    pub fn to_xonly_pub(&self) -> XOnlyPublicKey { XOnlyPublicKey::from(self.core.public_key) }

    /// Attempts to derive an extended public key from a path.
    ///
    /// The `path` argument can be any type implementing `AsRef<ChildNumber>`, such as
    /// `DerivationPath`, for instance.
    pub fn derive_pub(&self, path: impl AsRef<[NormalIndex]>) -> Self {
        let mut pk = *self;
        for cnum in path.as_ref() {
            pk = pk.ckd_pub(*cnum)
        }
        pk
    }

    /// Compute the scalar tweak added to this key to get a child key
    pub fn ckd_pub_tweak(&self, child_no: NormalIndex) -> (secp256k1::Scalar, ChainCode) {
        let mut hmac_engine: HmacEngine<sha512::Hash> =
            HmacEngine::new(self.core.chain_code.as_ref());
        hmac_engine.input(&self.core.public_key.serialize());
        hmac_engine.input(&child_no.to_be_bytes());

        let hmac_result: Hmac<sha512::Hash> = Hmac::from_engine(hmac_engine);

        let private_key = secp256k1::SecretKey::from_slice(&hmac_result[..32])
            .expect("negligible probability")
            .into();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hmac_result[32..]);
        let chain_code = ChainCode::from_raw_array(bytes);
        (private_key, chain_code)
    }

    /// Public->Public child key derivation
    pub fn ckd_pub(&self, child_no: NormalIndex) -> Xpub {
        let (scalar, chain_code) = self.ckd_pub_tweak(child_no);
        let tweaked =
            self.core.public_key.add_exp_tweak(SECP256K1, &scalar).expect("negligible probability");

        let meta = XpubMeta {
            depth: self.meta.depth + 1,
            parent_fp: self.fingerprint(),
            child_number: child_no.into(),
        };
        let core = XpubCore {
            public_key: tweaked,
            chain_code,
        };
        Xpub {
            testnet: self.testnet,
            meta,
            core,
        }
    }
}

impl Display for Xpub {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        base58::encode_check_to_fmt(f, &self.encode())
    }
}

impl FromStr for Xpub {
    type Err = XpubParseError;

    fn from_str(inp: &str) -> Result<Xpub, XpubParseError> {
        let data = base58::decode_check(inp)?;
        Ok(Xpub::decode(data)?)
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Display)]
#[display("{master_fp}{derivation}", alt = "{master_fp}{derivation:#}")]
pub struct XpubOrigin {
    master_fp: XpubFp,
    derivation: DerivationPath,
}

impl FromStr for XpubOrigin {
    type Err = XpubParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (master_fp, path) = match s.split_once('/') {
            None => (XpubFp::default(), ""),
            Some(("00000000", p)) | Some(("m", p)) => (XpubFp::default(), p),
            Some((fp, p)) => (XpubFp::from_str(fp)?, p),
        };
        Ok(XpubOrigin {
            master_fp,
            derivation: DerivationPath::from_str(path)?,
        })
    }
}

#[derive(Getters, Clone, Eq, PartialEq, Hash, Debug, Display)]
#[display("[{origin}]{xpub}", alt = "[{origin:#}]{xpub}")]
pub struct XpubDescriptor {
    origin: XpubOrigin,
    xpub: Xpub,
}

impl FromStr for XpubDescriptor {
    type Err = XpubParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with('[') {
            return Err(XpubParseError::NoOrigin);
        }
        let (origin, xpub) =
            s.trim_start_matches('[').split_once(']').ok_or(XpubParseError::NoOrigin)?;
        let d = XpubDescriptor {
            origin: XpubOrigin::from_str(origin)?,
            xpub: Xpub::from_str(xpub)?,
        };
        if d.origin.derivation.len() != d.xpub.meta.depth as usize {
            return Err(XpubParseError::DepthMismatch);
        }
        if !d.origin.derivation.is_empty() {
            let network = if d.xpub.testnet { HardenedIndex::ONE } else { HardenedIndex::ZERO };
            if d.origin.derivation.last() != Some(&network.into()) {
                return Err(XpubParseError::DepthMismatch);
            }
            if d.origin.derivation.last() != Some(&d.xpub.meta.child_number) {
                return Err(XpubParseError::DepthMismatch);
            }
        }
        Ok(d)
    }
}
