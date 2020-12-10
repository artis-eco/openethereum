use crate::contracts::validator_set::get_validator_pubkeys;
use crate::NodeId;
use client_traits::{EngineClient, TransactionRequest};
use common_types::ids::BlockId;
use engine::signer::EngineSigner;
use ethereum_types::{Address, H512, U256};
use hbbft::crypto::{PublicKeySet, SecretKeyShare};
use hbbft::sync_key_gen::{
	Ack, AckOutcome, Error, Part, PartOutcome, PubKeyMap, PublicKey, SecretKey, SyncKeyGen,
};
use hbbft::util::max_faulty;
use hbbft::NetworkInfo;
use itertools::Itertools;
use parity_crypto::publickey::Public;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use utils::bound_contract::{BoundContract, CallError};

use_contract!(key_history_contract, "res/key_history_contract.json");

lazy_static! {
	static ref KEYGEN_HISTORY_ADDRESS: Address =
		Address::from_str("7000000000000000000000000000000000000001").unwrap();
}

macro_rules! call_const_key_history {
	($c:ident, $x:ident $(, $a:expr )*) => {
		$c.call_const(key_history_contract::functions::$x::call($($a),*))
	};
}

pub fn engine_signer_to_synckeygen<'a>(
	signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
	pub_keys: PubKeyMap<Public, PublicWrapper>,
) -> Result<(SyncKeyGen<Public, PublicWrapper>, Option<Part>), Error> {
	let wrapper = KeyPairWrapper {
		inner: signer.clone(),
	};
	let public = match signer.read().as_ref() {
		Some(signer) => signer
			.public()
			.expect("Signer's public key must be available!"),
		None => Public::from(H512::from_low_u64_be(0)),
	};
	let mut rng = rand::thread_rng();
	let num_nodes = pub_keys.len();
	SyncKeyGen::new(public, wrapper, pub_keys, max_faulty(num_nodes), &mut rng)
}

pub fn synckeygen_to_network_info(
	synckeygen: &SyncKeyGen<Public, PublicWrapper>,
	pks: PublicKeySet,
	sks: Option<SecretKeyShare>,
) -> Option<NetworkInfo<NodeId>> {
	let pub_keys = synckeygen
		.public_keys()
		.keys()
		.map(|p| NodeId(*p))
		.collect::<Vec<_>>();
	println!("Creating Network Info");
	println!("pub_keys: {:?}", pub_keys);
	println!(
		"pks: {:?}",
		(0..(pub_keys.len()))
			.map(|i| pks.public_key_share(i))
			.collect::<Vec<_>>()
	);
	let sks = sks.unwrap();
	println!("sks.public_key_share: {:?}", sks.public_key_share());
	println!("sks.reveal: {:?}", sks.reveal());

	Some(NetworkInfo::new(
		NodeId(synckeygen.our_id().clone()),
		sks,
		pks,
		pub_keys,
	))
}

pub fn part_of_address(
	client: &dyn EngineClient,
	address: Address,
	vmap: &BTreeMap<Address, Public>,
	skg: &mut SyncKeyGen<Public, PublicWrapper>,
) -> Result<(), CallError> {
	let c = BoundContract::bind(client, BlockId::Latest, *KEYGEN_HISTORY_ADDRESS);
	let serialized_part = call_const_key_history!(c, parts, address)?;
	println!("Part for address {}: {:?}", address, serialized_part);
	if serialized_part.is_empty() {
		return Err(CallError::ReturnValueInvalid);
	}
	let deserialized_part: Part = bincode::deserialize(&serialized_part).unwrap();
	let mut rng = rand::thread_rng();
	let outcome = skg
		.handle_part(vmap.get(&address).unwrap(), deserialized_part, &mut rng)
		.unwrap();
	if let PartOutcome::Invalid(fault) = outcome {
		panic!("Expected Part Outcome to be valid. {}", fault);
	}
	Ok(())
}

pub fn acks_of_address(
	client: &dyn EngineClient,
	address: Address,
	vmap: &BTreeMap<Address, Public>,
	skg: &mut SyncKeyGen<Public, PublicWrapper>,
) -> Result<(), CallError> {
	let c = BoundContract::bind(client, BlockId::Latest, *KEYGEN_HISTORY_ADDRESS);
	let serialized_length = call_const_key_history!(c, get_acks_length, address)?;

	println!(
		"Acks for address {} is of size: {:?}",
		address, serialized_length
	);
	for n in 0..serialized_length.low_u64() {
		let serialized_ack = call_const_key_history!(c, acks, address, n)?;
		println!("Ack #{} for address {}: {:?}", n, address, serialized_ack);
		if serialized_ack.is_empty() {
			return Err(CallError::ReturnValueInvalid);
		}
		let deserialized_ack: Ack = bincode::deserialize(&serialized_ack).unwrap();
		let outcome = skg
			.handle_ack(vmap.get(&address).unwrap(), deserialized_ack)
			.unwrap();
		if let AckOutcome::Invalid(fault) = outcome {
			panic!("Expected Ack Outcome to be valid. {}", fault);
		}
	}

	Ok(())
}

#[derive(Clone)]
pub struct PublicWrapper {
	pub inner: Public,
}

#[derive(Clone)]
pub struct KeyPairWrapper {
	pub inner: Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
}

impl<'a> PublicKey for PublicWrapper {
	type Error = parity_crypto::publickey::Error;
	type SecretKey = KeyPairWrapper;
	fn encrypt<M: AsRef<[u8]>, R: rand::Rng>(
		&self,
		msg: M,
		_rng: &mut R,
	) -> Result<Vec<u8>, Self::Error> {
		parity_crypto::publickey::ecies::encrypt(&self.inner, b"", msg.as_ref())
	}
}

impl<'a> SecretKey for KeyPairWrapper {
	type Error = parity_crypto::publickey::Error;
	fn decrypt(&self, ct: &[u8]) -> Result<Vec<u8>, Self::Error> {
		self.inner
			.read()
			.as_ref()
			.ok_or(parity_crypto::publickey::Error::InvalidSecretKey)
			.expect("Signer must be set!")
			.decrypt(b"", ct)
	}
}

/// Read available keygen data from the blockchain and initialize a SyncKeyGen instance with it.
pub fn initialize_synckeygen(
	client: &dyn EngineClient,
	signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
) -> Result<SyncKeyGen<Public, PublicWrapper>, CallError> {
	let vmap = get_validator_pubkeys(&*client)?;
	let pub_keys: BTreeMap<_, _> = vmap
		.values()
		.map(|p| (*p, PublicWrapper { inner: p.clone() }))
		.collect();

	// if synckeygen creation fails then either signer or validator pub keys are problematic.
	// Todo: We should expect up to f clients to write invalid pub keys. Report and re-start pending validator set selection.
	let (mut synckeygen, _) = engine_signer_to_synckeygen(signer, Arc::new(pub_keys))
		.map_err(|_| CallError::ReturnValueInvalid)?;

	for v in vmap.keys().sorted() {
		part_of_address(&*client, *v, &vmap, &mut synckeygen)?;
	}
	for v in vmap.keys().sorted() {
		acks_of_address(&*client, *v, &vmap, &mut synckeygen)?;
	}

	Ok(synckeygen)
}

/// Returns a collection of transactions the pending validator has to submit in order to
/// complete the keygen history contract data necessary to generate the next key and switch to the new validator set.
pub fn send_keygen_transactions(
	client: &dyn EngineClient,
	signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
) -> Result<(), CallError> {
	// If we have no signer there is nothing for us to send.
	let address = match signer.read().as_ref() {
		Some(signer) => signer.address(),
		None => return Err(CallError::ReturnValueInvalid),
	};

	let vmap = get_validator_pubkeys(&*client)?;
	let pub_keys: BTreeMap<_, _> = vmap
		.values()
		.map(|p| (*p, PublicWrapper { inner: p.clone() }))
		.collect();

	// if synckeygen creation fails then either signer or validator pub keys are problematic.
	// Todo: We should expect up to f clients to write invalid pub keys. Report and re-start pending validator set selection.
	let (mut synckeygen, part) = engine_signer_to_synckeygen(signer, Arc::new(pub_keys))
		.map_err(|_| CallError::ReturnValueInvalid)?;

	// If there is no part then we are not part of the pending validator set and there is nothing for us to do.
	let part_data = match part {
		Some(part) => part,
		None => return Err(CallError::ReturnValueInvalid),
	};

	// Check if we already sent our part.
	let part_sent = part_of_address(client, address, &vmap, &mut synckeygen);

	if let Err(CallError::ReturnValueInvalid) = part_sent {
		// let us send our part
		let full_client = client.as_full_client().ok_or(CallError::NotFullClient)?;

		let serialized_part = match bincode::serialize(&part_data) {
			Ok(part) => part,
			Err(_) => return Err(CallError::ReturnValueInvalid),
		};
		let write_part_data = key_history_contract::functions::write_part::call(serialized_part);

		let part_transaction = TransactionRequest::call(*KEYGEN_HISTORY_ADDRESS, write_part_data.0)
			.gas(U256::from(900_000))
			.nonce(full_client.latest_nonce(&address))
			.gas_price(U256::from(10000000000u64));
		full_client.transact_silently(part_transaction);
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use engine::signer::{from_keypair, EngineSigner};
	use parity_crypto::publickey::{KeyPair, Secret};
	use std::collections::BTreeMap;
	use std::sync::Arc;

	#[test]
	fn test_synckeygen_initialization() {
		// Create a keypair
		let secret =
			Secret::from_str("49c437676c600660905204e5f3710a6db5d3f46e3da9ba5168b9d34b0b787317")
				.unwrap();
		let keypair = KeyPair::from_secret(secret).expect("KeyPair generation must succeed");
		let public = keypair.public().clone();
		let wrapper = PublicWrapper {
			inner: public.clone(),
		};

		// Convert it to a EngineSigner trait object
		let signer: Arc<RwLock<Option<Box<dyn EngineSigner>>>> =
			Arc::new(RwLock::new(Some(from_keypair(keypair))));

		// Initialize SyncKeyGen with the EngineSigner wrapper
		let mut pub_keys: BTreeMap<Public, PublicWrapper> = BTreeMap::new();
		pub_keys.insert(public, wrapper);

		assert!(engine_signer_to_synckeygen(&signer, Arc::new(pub_keys)).is_ok());
	}
}
