use log::*;

use enclave_ffi_types::EnclaveError;
use proto::tx::signing::SignMode;
use protobuf::Message;
use serde::{Deserialize, Serialize};

use crate::multisig::MultisigThresholdPubKey;

use enclave_crypto::{
    hash::sha::HASH_SIZE, secp256k1::Secp256k1PubKey, sha_256, traits::VerifyingKey, CryptoError,
};

use cosmos_proto as proto;

use cw_types_v010::{
    coins::Coin,
    encoding::Binary,
    math::Uint128,
    types::{CanonicalAddr, HumanAddr},
};

use crate::traits::CosmosAminoPubkey;

pub fn calc_contract_hash(contract_bytes: &[u8]) -> [u8; HASH_SIZE] {
    sha_256(contract_bytes)
}

pub struct ContractCode<'code> {
    code: &'code [u8],
    hash: [u8; HASH_SIZE],
}

impl<'code> ContractCode<'code> {
    pub fn new(code: &'code [u8]) -> Self {
        let hash = calc_contract_hash(code);
        Self { code, hash }
    }

    pub fn code(&self) -> &[u8] {
        self.code
    }

    pub fn hash(&self) -> [u8; HASH_SIZE] {
        self.hash
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum CosmosPubKey {
    Secp256k1(Secp256k1PubKey),
    Multisig(MultisigThresholdPubKey),
}

/// `"/"` + `proto::crypto::multisig::LegacyAminoPubKey::descriptor_static().full_name()`
const TYPE_URL_MULTISIG_LEGACY_AMINO_PUBKEY: &str = "/cosmos.crypto.multisig.LegacyAminoPubKey";
/// `"/"` + `proto::crypto::secp256k1::PubKey::descriptor_static().full_name()`
const TYPE_URL_SECP256K1_PUBKEY: &str = "/cosmos.crypto.secp256k1.PubKey";

impl CosmosPubKey {
    pub fn from_proto(public_key: &protobuf::well_known_types::Any) -> Result<Self, CryptoError> {
        let public_key_parser = match public_key.type_url.as_str() {
            TYPE_URL_SECP256K1_PUBKEY => Self::secp256k1_from_proto,
            TYPE_URL_MULTISIG_LEGACY_AMINO_PUBKEY => Self::multisig_legacy_amino_from_proto,
            _ => {
                warn!("found public key of unsupported type: {:?}", public_key);
                return Err(CryptoError::ParsingError);
            }
        };

        public_key_parser(&public_key.value)
    }

    fn secp256k1_from_proto(public_key_bytes: &[u8]) -> Result<Self, CryptoError> {
        use proto::crypto::secp256k1::PubKey;
        let pub_key = PubKey::parse_from_bytes(public_key_bytes).map_err(|_err| {
            warn!(
                "Could not parse secp256k1 public key from these bytes: {}",
                Binary(public_key_bytes.to_vec())
            );
            CryptoError::ParsingError
        })?;
        Ok(CosmosPubKey::Secp256k1(Secp256k1PubKey::new(pub_key.key)))
    }

    fn multisig_legacy_amino_from_proto(public_key_bytes: &[u8]) -> Result<Self, CryptoError> {
        use proto::crypto::multisig::LegacyAminoPubKey;
        let multisig_key =
            LegacyAminoPubKey::parse_from_bytes(public_key_bytes).map_err(|_err| {
                warn!(
                    "Could not parse multisig public key from these bytes: {}",
                    Binary(public_key_bytes.to_vec())
                );
                CryptoError::ParsingError
            })?;
        let mut pubkeys = vec![];
        for public_key in &multisig_key.public_keys {
            pubkeys.push(CosmosPubKey::from_proto(public_key)?);
        }
        Ok(CosmosPubKey::Multisig(MultisigThresholdPubKey::new(
            multisig_key.threshold,
            pubkeys,
        )))
    }
}

impl CosmosAminoPubkey for CosmosPubKey {
    fn get_address(&self) -> CanonicalAddr {
        match self {
            CosmosPubKey::Secp256k1(pubkey) => pubkey.get_address(),
            CosmosPubKey::Multisig(pubkey) => pubkey.get_address(),
        }
    }

    fn amino_bytes(&self) -> Vec<u8> {
        match self {
            CosmosPubKey::Secp256k1(pubkey) => pubkey.amino_bytes(),
            CosmosPubKey::Multisig(pubkey) => pubkey.amino_bytes(),
        }
    }
}

impl VerifyingKey for CosmosPubKey {
    fn verify_bytes(
        &self,
        bytes: &[u8],
        sig: &[u8],
        sign_mode: SignMode,
    ) -> Result<(), CryptoError> {
        match self {
            CosmosPubKey::Secp256k1(pubkey) => pubkey.verify_bytes(bytes, sig, sign_mode),
            CosmosPubKey::Multisig(pubkey) => pubkey.verify_bytes(bytes, sig, sign_mode),
        }
    }
}

// This type is a copy of the `proto::tx::signing::SignMode` allowing us
// to create a Deserialize impl for it without touching the autogenerated type.
// See: https://serde.rs/remote-derive.html
#[allow(non_camel_case_types)]
#[derive(Deserialize)]
#[serde(remote = "proto::tx::signing::SignMode")]
pub enum SignModeDef {
    SIGN_MODE_UNSPECIFIED = 0,
    SIGN_MODE_DIRECT = 1,
    SIGN_MODE_TEXTUAL = 2,
    SIGN_MODE_LEGACY_AMINO_JSON = 127,
    SIGN_MODE_EIP_191 = 191,
}

#[allow(non_camel_case_types)]
#[derive(Deserialize, Clone, Debug, PartialEq, Copy)]
pub enum HandleType {
    HANDLE_TYPE_EXECUTE = 0,
    HANDLE_TYPE_REPLY = 1,
    HANDLE_TYPE_IBC_CHANNEL_OPEN = 2,
    HANDLE_TYPE_IBC_CHANNEL_CONNECT = 3,
    HANDLE_TYPE_IBC_CHANNEL_CLOSE = 4,
    HANDLE_TYPE_IBC_PACKET_RECEIVE = 5,
    HANDLE_TYPE_IBC_PACKET_ACK = 6,
    HANDLE_TYPE_IBC_PACKET_TIMEOUT = 7,
    HANDLE_TYPE_IBC_WASM_HOOKS_INCOMING_TRANSFER = 8,
    HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_ACK = 9,
    HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_TIMEOUT = 10,
}

impl HandleType {
    pub fn try_from(value: u8) -> Result<Self, EnclaveError> {
        match value {
            0 => Ok(HandleType::HANDLE_TYPE_EXECUTE),
            1 => Ok(HandleType::HANDLE_TYPE_REPLY),
            2 => Ok(HandleType::HANDLE_TYPE_IBC_CHANNEL_OPEN),
            3 => Ok(HandleType::HANDLE_TYPE_IBC_CHANNEL_CONNECT),
            4 => Ok(HandleType::HANDLE_TYPE_IBC_CHANNEL_CLOSE),
            5 => Ok(HandleType::HANDLE_TYPE_IBC_PACKET_RECEIVE),
            6 => Ok(HandleType::HANDLE_TYPE_IBC_PACKET_ACK),
            7 => Ok(HandleType::HANDLE_TYPE_IBC_PACKET_TIMEOUT),
            8 => Ok(HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_INCOMING_TRANSFER),
            9 => Ok(HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_ACK),
            10 => Ok(HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_TIMEOUT),
            _ => {
                error!("unrecognized handle type: {}", value);
                Err(EnclaveError::FailedToDeserialize)
            }
        }
    }

    pub fn get_export_name(h: &HandleType) -> &'static str {
        match h {
            HandleType::HANDLE_TYPE_EXECUTE => "execute",
            HandleType::HANDLE_TYPE_REPLY => "reply",
            HandleType::HANDLE_TYPE_IBC_CHANNEL_OPEN => "ibc_channel_open",
            HandleType::HANDLE_TYPE_IBC_CHANNEL_CONNECT => "ibc_channel_connect",
            HandleType::HANDLE_TYPE_IBC_CHANNEL_CLOSE => "ibc_channel_close",
            HandleType::HANDLE_TYPE_IBC_PACKET_RECEIVE => "ibc_packet_receive",
            HandleType::HANDLE_TYPE_IBC_PACKET_ACK => "ibc_packet_ack",
            HandleType::HANDLE_TYPE_IBC_PACKET_TIMEOUT => "ibc_packet_timeout",
            HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_INCOMING_TRANSFER => "execute",
            HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_ACK => "sudo",
            HandleType::HANDLE_TYPE_IBC_WASM_HOOKS_OUTGOING_TRANSFER_TIMEOUT => "sudo",
        }
    }
}

#[allow(non_camel_case_types)]
#[derive(Deserialize, Clone, Debug, PartialEq, Copy)]
pub enum VerifyParamsType {
    HandleType(HandleType),
    Init,
    Migrate,
    /// UpdateAdmin is used both for updating the admin and clearing the admin
    /// (by passing an empty admin address)
    UpdateAdmin,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct SigInfo {
    pub tx_bytes: Binary,
    pub sign_bytes: Binary,
    #[serde(with = "SignModeDef")]
    pub sign_mode: proto::tx::signing::SignMode,
    pub mode_info: Binary,
    pub public_key: Binary,
    pub signature: Binary,
    pub callback_sig: Option<Binary>,
}

// Should be in sync with https://github.com/cosmos/cosmos-sdk/blob/v0.38.3/x/auth/types/stdtx.go#L216
#[derive(Deserialize, Clone, Default, Debug, PartialEq)]
pub struct StdSignDoc {
    pub account_number: String,
    pub chain_id: String,
    pub memo: String,
    pub msgs: Vec<AminoSdkMsg>,
    pub sequence: String,
}

#[derive(Debug)]
pub struct SignDoc {
    pub body: TxBody,
    pub auth_info: AuthInfo,
    pub chain_id: String,
    pub account_number: u64,
}

impl SignDoc {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EnclaveError> {
        let raw_sign_doc = proto::tx::tx::SignDoc::parse_from_bytes(bytes).map_err(|err| {
            warn!(
                "got an error while trying to deserialize sign doc bytes from protobuf: {}: {}",
                err,
                Binary(bytes.into()),
            );
            EnclaveError::FailedToDeserialize
        })?;

        let body = TxBody::from_bytes(&raw_sign_doc.body_bytes)?;
        let auth_info = AuthInfo::from_bytes(&raw_sign_doc.auth_info_bytes)?;

        Ok(Self {
            body,
            auth_info,
            chain_id: raw_sign_doc.chain_id,
            account_number: raw_sign_doc.account_number,
        })
    }
}

#[derive(Debug)]
pub struct TxBody {
    pub messages: Vec<DirectSdkMsg>,
    // Leaving this here for discoverability. We can use this, but don't verify it today.
    #[allow(dead_code)]
    memo: (),
    #[allow(dead_code)]
    timeout_height: (),
}

impl TxBody {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EnclaveError> {
        let tx_body = proto::tx::tx::TxBody::parse_from_bytes(bytes).map_err(|err| {
            warn!(
                "got an error while trying to deserialize cosmos message body bytes from protobuf: {}: {}",
                err,
                Binary(bytes.into()),
            );
            EnclaveError::FailedToDeserialize
        })?;

        let messages = tx_body
            .messages
            .into_iter()
            .map(|any| DirectSdkMsg::from_bytes(&any.type_url, &any.value))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(TxBody {
            messages,
            memo: (),
            timeout_height: (),
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum AminoSdkMsg {
    #[serde(alias = "wasm/MsgExecuteContract")]
    Execute {
        sender: HumanAddr,
        contract: HumanAddr,
        /// msg is the json-encoded HandleMsg struct (as raw Binary)
        msg: String,
        sent_funds: Vec<Coin>,
    },
    #[serde(alias = "wasm/MsgInstantiateContract")]
    Instantiate {
        sender: HumanAddr,
        code_id: String,
        init_msg: String,
        init_funds: Vec<Coin>,
        label: String,
        #[serde(default)]
        admin: HumanAddr,
    },
    #[serde(alias = "wasm/MsgMigrateContract")]
    Migrate {
        sender: HumanAddr,
        contract: HumanAddr,
        code_id: String,
        msg: String,
    },
    #[serde(alias = "wasm/MsgUpdateAdmin")]
    MsgUpdateAdmin {
        sender: HumanAddr,
        new_admin: HumanAddr,
        contract: HumanAddr,
    },
    #[serde(alias = "wasm/MsgClearAdmin")]
    MsgClearAdmin {
        sender: HumanAddr,
        contract: HumanAddr,
    },
    // The core IBC messages don't support Amino
    #[serde(other, deserialize_with = "deserialize_ignore_any")]
    Other,
}

pub fn deserialize_ignore_any<'de, D: serde::Deserializer<'de>, T: Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| T::default())
}

impl AminoSdkMsg {
    pub fn into_direct_msg(self) -> Result<DirectSdkMsg, EnclaveError> {
        match self {
            Self::Migrate {
                sender,
                msg,
                contract,
                code_id,
            } => {
                let sender = CanonicalAddr::from_human(&sender).map_err(|err| {
                    warn!("failed to turn human addr to canonical addr when parsing DirectSdkMsg: {:?}", err);
                    EnclaveError::FailedToDeserialize
                })?;
                let msg = Binary::from_base64(&msg).map_err(|err| {
                    warn!(
                        "failed to parse base64 msg when parsing DirectSdkMsg: {:?}",
                        err
                    );
                    EnclaveError::FailedToDeserialize
                })?;
                let msg = msg.0;
                let code_id = code_id.parse::<u64>().map_err(|err| {
                    warn!(
                        "failed to parse code_id as u64 when parsing DirectSdkMsg: {:?}",
                        err
                    );
                    EnclaveError::FailedToDeserialize
                })?;

                Ok(DirectSdkMsg::MsgMigrateContract {
                    sender,
                    msg,
                    contract,
                    code_id,
                })
            }
            Self::Execute {
                sender,
                contract,
                msg,
                sent_funds,
            } => {
                let sender = CanonicalAddr::from_human(&sender).map_err(|err| {
                    warn!("failed to turn human addr to canonical addr when parsing DirectSdkMsg: {:?}", err);
                    EnclaveError::FailedToDeserialize
                })?;
                let msg = Binary::from_base64(&msg).map_err(|err| {
                    warn!(
                        "failed to parse base64 msg when parsing DirectSdkMsg: {:?}",
                        err
                    );
                    EnclaveError::FailedToDeserialize
                })?;
                let msg = msg.0;

                Ok(DirectSdkMsg::MsgExecuteContract {
                    sender,
                    contract,
                    msg,
                    sent_funds,
                })
            }
            Self::Instantiate {
                sender,
                init_msg,
                init_funds,
                label,
                code_id,
                admin,
            } => {
                let sender = CanonicalAddr::from_human(&sender).map_err(|err| {
                    warn!("failed to turn human addr to canonical addr when parsing DirectSdkMsg: {:?}", err);
                    EnclaveError::FailedToDeserialize
                })?;
                let init_msg = Binary::from_base64(&init_msg).map_err(|err| {
                    warn!(
                        "failed to parse base64 init_msg when parsing DirectSdkMsg: {:?}",
                        err
                    );
                    EnclaveError::FailedToDeserialize
                })?;
                let init_msg = init_msg.0;
                let code_id = code_id.parse::<u64>().map_err(|err| {
                    warn!(
                        "failed to parse code_id as u64 when parsing DirectSdkMsg: {:?}",
                        err
                    );
                    EnclaveError::FailedToDeserialize
                })?;

                Ok(DirectSdkMsg::MsgInstantiateContract {
                    sender,
                    code_id,
                    init_msg,
                    init_funds,
                    label,
                    admin,
                })
            }
            AminoSdkMsg::MsgUpdateAdmin {
                sender,
                new_admin,
                contract,
            } => {
                let sender = CanonicalAddr::from_human(&sender).map_err(|err| {
                    warn!("failed to turn human addr to canonical addr when parsing DirectSdkMsg: {:?}", err);
                    EnclaveError::FailedToDeserialize
                })?;

                Ok(DirectSdkMsg::MsgUpdateAdmin {
                    sender,
                    new_admin,
                    contract,
                })
            }
            AminoSdkMsg::MsgClearAdmin { sender, contract } => {
                let sender = CanonicalAddr::from_human(&sender).map_err(|err| {
                    warn!("failed to turn human addr to canonical addr when parsing DirectSdkMsg: {:?}", err);
                    EnclaveError::FailedToDeserialize
                })?;

                Ok(DirectSdkMsg::MsgClearAdmin { sender, contract })
            }
            Self::Other => Ok(DirectSdkMsg::Other),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]

pub struct FungibleTokenPacketData {
    pub denom: String,
    pub amount: Uint128,
    pub sender: HumanAddr,
    pub receiver: HumanAddr,
    pub memo: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IbcHooksIncomingTransferMsg {
    pub wasm: IbcHooksIncomingTransferWasmMsg,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]

pub struct IbcHooksIncomingTransferWasmMsg {
    pub contract: HumanAddr,
    pub msg: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IbcHooksOutgoingTransferMemo {
    pub ibc_callback: HumanAddr,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Height {
    pub revision_number: u64,
    pub revision_height: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IBCLifecycleComplete {
    #[serde(rename = "ibc_lifecycle_complete")]
    IBCLifecycleComplete(IBCLifecycleCompleteOptions),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum IBCLifecycleCompleteOptions {
    #[serde(rename = "ibc_ack")]
    IBCAck {
        /// The source channel (Secret side) of the IBC packet
        channel: String,
        /// The sequence number that the packet was sent with
        sequence: u64,
        /// String encoded version of the ack as seen by OnAcknowledgementPacket(..)
        ack: String,
        /// Weather an ack is a success of failure according to the transfer spec
        success: bool,
    },
    #[serde(rename = "ibc_timeout")]
    IBCTimeout {
        /// The source channel (secret side) of the IBC packet
        channel: String,
        /// The sequence number that the packet was sent with
        sequence: u64,
    },
}

pub fn is_transfer_ack_error(acknowledgement: &[u8]) -> bool {
    match serde_json::from_slice::<AcknowledgementError>(acknowledgement) {
        Ok(ack_err) => {
            if ack_err.error.is_some() {
                return true;
            }
        }
        Err(_err) => {}
    }
    false
}

#[derive(Deserialize, Debug)]
pub struct AcknowledgementError {
    pub error: Option<String>,
}

// // This is needed to make sure that fields other than error are ignored as we don't care about them
// impl Default for AcknowledgementError {
//     fn default() -> Self {
//         Self { error: None }
//     }
// }

#[derive(Debug, Deserialize)]
pub struct IBCPacketAckMsg {
    pub acknowledgement: IBCAcknowledgement,
    pub original_packet: IBCPacket,
    pub relayer: String,
}

#[derive(Debug, Deserialize)]
pub struct IBCAcknowledgement {
    pub data: Binary,
}

#[derive(Debug, Deserialize)]
pub struct IncentivizedAcknowledgement {
    pub app_acknowledgement: Binary,
    pub forward_relayer_address: String,
    pub underlying_app_success: bool,
}

#[derive(Debug, Deserialize)]
pub struct IBCPacketTimeoutMsg {
    pub packet: IBCPacket,
    pub relayer: String,
}

#[derive(Debug, Deserialize)]
pub struct IBCPacket {
    pub data: Binary,
    pub src: IBCEndpoint,
    pub dest: IBCEndpoint,
    pub sequence: u64,
    pub timeout: IBCTimeout,
}

#[derive(Debug, Deserialize)]
pub struct IBCEndpoint {
    pub port_id: String,
    pub channel_id: String,
}

#[derive(Debug, Deserialize)]
pub struct IBCTimeout {
    pub block: Option<IBCTimeoutBlock>,
    pub timestamp: Option<Uint128>,
}

#[derive(Debug, Deserialize)]
pub struct IBCTimeoutBlock {
    pub revision: u64,
    pub height: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub sequence: u64,
    pub source_port: String,
    pub source_channel: String,
    /// if the packet is sent into an IBC-enabled contract, `destination_port` will be `"wasm.{contract_address}"`
    /// if the packet is rounted here via ibc-hooks, `destination_port` will be `"transfer"`
    pub destination_port: String,
    pub destination_channel: String,
    /// if the packet is sent into an IBC-enabled contract, this will be raw bytes
    /// if the packet is rounted here via ibc-hooks, this will be a JSON string of the type `FungibleTokenPacketData` (https://github.com/cosmos/ibc-go/blob/v4.3.0/modules/apps/transfer/types/packet.pb.go#L25-L39)
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DirectSdkMsg {
    // CosmWasm:
    MsgExecuteContract {
        sender: CanonicalAddr,
        contract: HumanAddr,
        msg: Vec<u8>,
        sent_funds: Vec<Coin>,
    },
    MsgInstantiateContract {
        sender: CanonicalAddr,
        init_msg: Vec<u8>,
        init_funds: Vec<Coin>,
        label: String,
        admin: HumanAddr,
        code_id: u64,
    },
    MsgMigrateContract {
        sender: CanonicalAddr,
        contract: HumanAddr,
        msg: Vec<u8>,
        code_id: u64,
    },
    MsgUpdateAdmin {
        sender: CanonicalAddr,
        new_admin: HumanAddr,
        contract: HumanAddr,
    },
    MsgClearAdmin {
        sender: CanonicalAddr,
        contract: HumanAddr,
    },
    // IBC:
    // MsgChannelOpenInit {}, // TODO
    // MsgChannelOpenTry {}, // TODO
    // MsgChannelOpenAck {}, // TODO
    // MsgChannelOpenConfirm {}, // TODO
    // MsgChannelCloseInit {}, // TODO
    // MsgChannelCloseConfirm {}, // TODO
    MsgAcknowledgement {
        packet: Packet,
        acknowledgement: Vec<u8>,
        proof_acked: Vec<u8>,
        proof_height: Option<Height>,
        signer: String,
    },
    MsgTimeout {
        packet: Packet,
        proof_unreceived: Vec<u8>,
        proof_height: Option<Height>,
        next_sequence_recv: u64,
        signer: String,
    },
    MsgRecvPacket {
        packet: Packet,
        proof_commitment: Vec<u8>,
        proof_height: Option<Height>,
        signer: String,
    },
    // All else:
    Other,
}

impl DirectSdkMsg {
    pub fn from_bytes(type_url: &str, bytes: &[u8]) -> Result<Self, EnclaveError> {
        match type_url {
            "/secret.compute.v1beta1.MsgInstantiateContract" => Self::try_parse_instantiate(bytes),
            "/secret.compute.v1beta1.MsgExecuteContract" => Self::try_parse_execute(bytes),
            "/secret.compute.v1beta1.MsgMigrateContract" => Self::try_parse_migrate(bytes),
            "/secret.compute.v1beta1.MsgUpdateAdmin" => Self::try_parse_update_admin(bytes),
            "/secret.compute.v1beta1.MsgClearAdmin" => Self::try_parse_clear_admin(bytes),
            "/ibc.core.channel.v1.MsgRecvPacket" => Self::try_parse_ibc_recv_packet(bytes),
            "/ibc.core.channel.v1.MsgAcknowledgement" => Self::try_parse_ibc_ack(bytes),
            "/ibc.core.channel.v1.MsgTimeout" => Self::try_parse_ibc_timeout(bytes),
            _ => Ok(DirectSdkMsg::Other),
        }
    }

    // fn try_parse_msg_channel_open_init(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    // fn try_parse_msg_channel_open_try(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    // fn try_parse_msg_channel_open_ack(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    // fn try_parse_msg_channel_open_confirm(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    // fn try_parse_msg_channel_close_init(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    // fn try_parse_msg_channel_close_confirm(bytes: &[u8]) -> Result<Self, EnclaveError> {
    //     todo!()
    // }

    fn try_parse_ibc_ack(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::ibc::tx::MsgAcknowledgement;

        let raw_msg = MsgAcknowledgement::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        match raw_msg.packet.clone().into_option() {
            None => Err(EnclaveError::FailedToDeserialize),
            Some(packet) => Ok(DirectSdkMsg::MsgAcknowledgement {
                packet: Packet {
                    sequence: packet.sequence,
                    source_port: packet.source_port,
                    source_channel: packet.source_channel,
                    destination_port: packet.destination_port,
                    destination_channel: packet.destination_channel,
                    data: packet.data,
                },
                acknowledgement: raw_msg.acknowledgement,
                proof_acked: raw_msg.proof_acked,
                proof_height: raw_msg.proof_height.into_option().map(|height| Height {
                    revision_number: height.revision_number,
                    revision_height: height.revision_height,
                }),
                signer: raw_msg.signer,
            }),
        }
    }

    fn try_parse_ibc_timeout(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::ibc::tx::MsgTimeout;

        let raw_msg =
            MsgTimeout::parse_from_bytes(bytes).map_err(|_| EnclaveError::FailedToDeserialize)?;

        match raw_msg.packet.clone().into_option() {
            None => Err(EnclaveError::FailedToDeserialize),
            Some(packet) => Ok(DirectSdkMsg::MsgTimeout {
                packet: Packet {
                    sequence: packet.sequence,
                    source_port: packet.source_port,
                    source_channel: packet.source_channel,
                    destination_port: packet.destination_port,
                    destination_channel: packet.destination_channel,
                    data: packet.data,
                },
                next_sequence_recv: raw_msg.next_sequence_recv,
                proof_unreceived: raw_msg.proof_unreceived,
                proof_height: raw_msg.proof_height.into_option().map(|height| Height {
                    revision_number: height.revision_number,
                    revision_height: height.revision_height,
                }),
                signer: raw_msg.signer,
            }),
        }
    }

    fn try_parse_ibc_recv_packet(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::ibc::tx::MsgRecvPacket;

        let raw_msg = MsgRecvPacket::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        match raw_msg.packet.into_option() {
            None => Err(EnclaveError::FailedToDeserialize),
            Some(packet) => Ok(DirectSdkMsg::MsgRecvPacket {
                packet: Packet {
                    sequence: packet.sequence,
                    source_port: packet.source_port,
                    source_channel: packet.source_channel,
                    destination_port: packet.destination_port,
                    destination_channel: packet.destination_channel,
                    data: packet.data,
                },
                proof_commitment: raw_msg.proof_commitment,
                proof_height: raw_msg.proof_height.into_option().map(|height| Height {
                    revision_number: height.revision_number,
                    revision_height: height.revision_height,
                }),
                signer: raw_msg.signer,
            }),
        }
    }

    fn try_parse_migrate(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::cosmwasm::msg::MsgMigrateContract;

        let raw_msg = MsgMigrateContract::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        trace!(
            "try_parse_migrate sender: len={} val={:?}",
            raw_msg.sender.len(),
            raw_msg.sender
        );

        let sender = CanonicalAddr::from_human(&HumanAddr(raw_msg.sender))
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        Ok(DirectSdkMsg::MsgMigrateContract {
            sender,
            msg: raw_msg.msg,
            contract: HumanAddr(raw_msg.contract),
            code_id: raw_msg.code_id,
        })
    }

    fn try_parse_update_admin(bytes: &[u8]) -> Result<Self, EnclaveError> {
        let raw_msg = proto::cosmwasm::msg::MsgUpdateAdmin::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        trace!(
            "try_parse_update_admin sender: len={} val={:?}",
            raw_msg.sender.len(),
            raw_msg.sender
        );

        let sender = CanonicalAddr::from_human(&HumanAddr(raw_msg.sender))
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        let new_admin = HumanAddr(raw_msg.new_admin);

        Ok(DirectSdkMsg::MsgUpdateAdmin {
            sender,
            new_admin,
            contract: HumanAddr(raw_msg.contract),
        })
    }

    fn try_parse_clear_admin(bytes: &[u8]) -> Result<Self, EnclaveError> {
        let raw_update_msg = proto::cosmwasm::msg::MsgClearAdmin::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        trace!(
            "try_parse_clear_admin sender: len={} val={:?}",
            raw_update_msg.sender.len(),
            raw_update_msg.sender
        );

        let sender = CanonicalAddr::from_human(&HumanAddr(raw_update_msg.sender))
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        Ok(DirectSdkMsg::MsgClearAdmin {
            sender,
            contract: HumanAddr(raw_update_msg.contract),
        })
    }

    fn try_parse_instantiate(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::cosmwasm::msg::MsgInstantiateContract;

        let raw_msg = MsgInstantiateContract::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        trace!(
            "try_parse_instantiate sender: len={} val={:?}",
            raw_msg.sender.len(),
            raw_msg.sender
        );

        let init_funds = Self::parse_funds(raw_msg.init_funds)?;

        Ok(DirectSdkMsg::MsgInstantiateContract {
            sender: CanonicalAddr(Binary(raw_msg.sender)),
            init_msg: raw_msg.init_msg,
            init_funds,
            label: raw_msg.label,
            admin: HumanAddr(raw_msg.admin),
            code_id: raw_msg.code_id,
        })
    }

    fn try_parse_execute(bytes: &[u8]) -> Result<Self, EnclaveError> {
        use proto::cosmwasm::msg::MsgExecuteContract;

        let raw_msg = MsgExecuteContract::parse_from_bytes(bytes)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        trace!(
            "try_parse_execute sender: len={} val={:?}",
            raw_msg.sender.len(),
            raw_msg.sender
        );

        trace!(
            "try_parse_execute contract: len={} val={:?}",
            raw_msg.contract.len(),
            raw_msg.contract
        );

        // humanize address
        let contract = HumanAddr::from_canonical(&CanonicalAddr(Binary(raw_msg.contract)))
            .map_err(|err| {
                warn!(
                    "Contract address to execute was not a valid string: {}",
                    err,
                );
                EnclaveError::FailedToDeserialize
            })?;

        let sent_funds = Self::parse_funds(raw_msg.sent_funds)?;

        Ok(DirectSdkMsg::MsgExecuteContract {
            sender: CanonicalAddr(Binary(raw_msg.sender)),
            contract,
            msg: raw_msg.msg,
            sent_funds,
        })
    }

    fn parse_funds(
        raw_init_funds: protobuf::RepeatedField<proto::base::coin::Coin>,
    ) -> Result<Vec<Coin>, EnclaveError> {
        let mut init_funds = Vec::with_capacity(raw_init_funds.len());
        for raw_coin in raw_init_funds {
            let amount: u128 = raw_coin.amount.parse().map_err(|_err| {
                warn!(
                    "instantiate message funds were not a numeric string: {:?}",
                    raw_coin.amount,
                );
                EnclaveError::FailedToDeserialize
            })?;
            let coin = Coin {
                amount: Uint128(amount),
                denom: raw_coin.denom,
            };
            init_funds.push(coin);
        }

        Ok(init_funds)
    }

    pub fn sender(&self) -> Option<&CanonicalAddr> {
        match self {
            DirectSdkMsg::MsgExecuteContract { sender, .. }
            | DirectSdkMsg::MsgInstantiateContract { sender, .. }
            | DirectSdkMsg::MsgMigrateContract { sender, .. }
            | DirectSdkMsg::MsgUpdateAdmin { sender, .. }
            | DirectSdkMsg::MsgClearAdmin { sender, .. } => Some(sender),
            DirectSdkMsg::MsgRecvPacket { .. } => None,
            DirectSdkMsg::MsgAcknowledgement { .. } => None,
            DirectSdkMsg::MsgTimeout { .. } => None,
            DirectSdkMsg::Other => None,
        }
    }
}

#[derive(Debug)]
pub struct AuthInfo {
    pub signer_infos: Vec<SignerInfo>,
    // Leaving this here for discoverability. We can use this, but don't verify it today.
    #[allow(dead_code)]
    fee: (),
}

impl AuthInfo {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EnclaveError> {
        let raw_auth_info = proto::tx::tx::AuthInfo::parse_from_bytes(bytes).map_err(|err| {
            warn!("Could not parse AuthInfo from protobuf bytes: {:?}", err);
            EnclaveError::FailedToDeserialize
        })?;

        let mut signer_infos = vec![];
        for raw_signer_info in raw_auth_info.signer_infos {
            let signer_info = SignerInfo::from_proto(raw_signer_info)?;
            signer_infos.push(signer_info);
        }

        if signer_infos.is_empty() {
            warn!("No signature information provided for this TX. signer_infos empty");
            return Err(EnclaveError::FailedToDeserialize);
        }

        Ok(Self {
            signer_infos,
            fee: (),
        })
    }

    pub fn sender_public_key(&self, sender: &CanonicalAddr) -> Option<&CosmosPubKey> {
        self.signer_infos
            .iter()
            .find(|signer_info| &signer_info.public_key.get_address() == sender)
            .map(|si| &si.public_key)
    }
}

#[derive(Debug)]
pub struct SignerInfo {
    pub public_key: CosmosPubKey,
    pub sequence: u64,
}

impl SignerInfo {
    pub fn from_proto(raw_signer_info: proto::tx::tx::SignerInfo) -> Result<Self, EnclaveError> {
        if !raw_signer_info.has_public_key() {
            warn!("One of the provided signers had no public key");
            return Err(EnclaveError::FailedToDeserialize);
        }

        // unwraps valid after checks above
        let any_public_key = raw_signer_info.public_key.get_ref();

        let public_key = CosmosPubKey::from_proto(any_public_key)
            .map_err(|_| EnclaveError::FailedToDeserialize)?;

        let signer_info = Self {
            public_key,
            sequence: raw_signer_info.sequence,
        };
        Ok(signer_info)
    }
}
