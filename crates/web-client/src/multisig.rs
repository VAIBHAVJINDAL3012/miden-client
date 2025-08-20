use alloc::vec::Vec;

use wasm_bindgen::prelude::*;

use crate::{
    WebClient,
    models::{
        account::Account, account_id::AccountId, felt::FeltArray, public_key::PublicKey,
        transaction_request::TransactionRequest, transaction_result::TransactionResult,
        transaction_summary::TransactionSummary, word::Word,
    },
};

#[wasm_bindgen]
pub struct SetupMultisigAccountResult(Account, Word);

#[wasm_bindgen]
pub struct MaybeFeltList(Option<FeltArray>);

#[wasm_bindgen]
impl WebClient {
    #[wasm_bindgen(js_name = "setupAccount")]
    pub fn setup_multisig_account(
        &mut self,
        approvers: Vec<PublicKey>,
        threshold: u32,
    ) -> Result<SetupMultisigAccountResult, JsValue> {
        let Some(multisig_client) = self.get_mut_inner() else {
            return Err(JsValue::from_str("Multisig client not initialized"));
        };

        let approvers = approvers.into_iter().map(From::from).collect();

        let (multisig_account, seed) = multisig_client.setup_account(approvers, threshold);

        Ok(SetupMultisigAccountResult(multisig_account.into(), seed.into()))
    }

    #[wasm_bindgen(js_name = "proposeMultisigTransaction")]
    pub async fn propose_multisig_transaction(
        &mut self,
        account_id: AccountId,
        transaction_request: TransactionRequest,
    ) -> Result<TransactionSummary, JsValue> {
        let Some(multisig_client) = self.get_mut_inner() else {
            return Err(JsValue::from_str("Multisig client not initialized"));
        };

        multisig_client
            .propose_multisig_transaction(account_id.into(), transaction_request.into())
            .await
            .map(From::from)
            .map_err(|e| {
                JsValue::from_str(&format!("Failed to propose multisig transaction: {}", e))
            })
    }

    #[wasm_bindgen(js_name = "newMultisigTransaction")]
    pub async fn new_multisig_transaction(
        &mut self,
        account: Account,
        transaction_request: TransactionRequest,
        transaction_summary: TransactionSummary,
        signatures: Vec<MaybeFeltList>,
    ) -> Result<TransactionResult, JsValue> {
        let Some(multisig_client) = self.get_mut_inner() else {
            return Err(JsValue::from_str("Multisig client not initialized"));
        };

        multisig_client
            .new_multisig_transaction(
                account.into(),
                transaction_request.into(),
                transaction_summary.into(),
                signatures.into_iter().map(|s| s.0.map(From::from)).collect(),
            )
            .await
            .map(From::from)
            .map_err(|e| {
                JsValue::from_str(&format!("Failed to execute new multisig transaction: {}", e))
            })
    }
}
