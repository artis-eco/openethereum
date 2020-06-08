//! Utility functions.
//!
//! Contains small functions used by the AuRa engine that are not strictly limited to that scope.
//! TODO: Replace with the version in the PR https://github.com/paritytech/parity-ethereum/pull/10946
//!       once it is merged to master.

use std::fmt;

use client_traits::EngineClient;
use common_types::ids::BlockId;
use ethabi;
use ethereum_types::Address;

/// A contract bound to a client and block number.
///
/// A bound contract is a combination of a `Client` reference, a `BlockId` and a contract `Address`.
/// These three parts are enough to call a contract's function; return values are automatically
/// decoded.
pub struct BoundContract<'a> {
	client: &'a dyn EngineClient,
	block_id: BlockId,
	contract_addr: Address,
}

/// Contract call failed error.
#[derive(Debug)]
pub enum CallError {
	/// The call itself failed.
	CallFailed(String),
	/// Decoding the return value failed or the decoded value was a failure.
	DecodeFailed(ethabi::Error),
	/// The passed in client reference could not be upgraded to a `BlockchainClient`.
	NotFullClient,
	/// The value(s) returned by the call are unexpected
	ReturnValueInvalid,
}

impl<'a> fmt::Debug for BoundContract<'a> {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		fmt.debug_struct("BoundContract")
			.field("client", &(self.client as *const dyn EngineClient))
			.field("block_id", &self.block_id)
			.field("contract_addr", &self.contract_addr)
			.finish()
	}
}

impl<'a> BoundContract<'a> {
	/// Create a new `BoundContract`.
	#[inline]
	pub fn bind(
		client: &dyn EngineClient,
		block_id: BlockId,
		contract_addr: Address,
	) -> BoundContract {
		BoundContract {
			client,
			block_id,
			contract_addr,
		}
	}

	/// Perform a function call to an ethereum machine that doesn't create a transaction or change the state.
	///
	/// Runs a constant function call on `client`. The `call` value can be serialized by calling any
	/// api function generated by the `use_contract!` macro. This does not create any transactions, it only produces a
	/// result based on the state at the current block.
	pub fn call_const<D>(&self, call: (ethabi::Bytes, D)) -> Result<D::Output, CallError>
	where
		D: ethabi::FunctionOutputDecoder,
	{
		let (data, output_decoder) = call;

		let call_return = self
			.client
			.as_full_client()
			.ok_or(CallError::NotFullClient)?
			.call_contract(self.block_id, self.contract_addr, data)
			.map_err(CallError::CallFailed)?;

		// Decode the result and return it.
		output_decoder
			.decode(call_return.as_slice())
			.map_err(CallError::DecodeFailed)
	}
}
