use wasm_bindgen::prelude::*;

use super::note_id::NoteId;
use super::transaction_request::TransactionRequest;

/// Result of [`WebClient::build_pswap_request`].
///
/// Contains the assembled transaction request and an optional note ID for the
/// newly created pswap note (if the swap was only partially fulfilled or no
/// matching notes were found).
#[wasm_bindgen]
pub struct PswapRequestResult {
    request: TransactionRequest,
    pswap_note_id: Option<NoteId>,
}

impl PswapRequestResult {
    pub fn new(request: TransactionRequest, pswap_note_id: Option<NoteId>) -> Self {
        Self { request, pswap_note_id }
    }
}

#[wasm_bindgen]
impl PswapRequestResult {
    /// The assembled transaction request ready for execution.
    #[wasm_bindgen(getter, js_name = "transactionRequest")]
    pub fn transaction_request(&self) -> TransactionRequest {
        self.request.clone()
    }

    /// The note ID of the newly created pswap note, or `undefined` if the swap
    /// was fully fulfilled from existing notes.
    #[wasm_bindgen(getter, js_name = "pswapNoteId")]
    pub fn pswap_note_id(&self) -> Option<NoteId> {
        self.pswap_note_id
    }
}
