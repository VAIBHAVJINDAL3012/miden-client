use alloc::vec::Vec;

use miden_protocol::{
    Felt, Word,
    account::AccountId,
    asset::{Asset, FungibleAsset},
    note::{Note, NoteAttachment, NoteDetails, NoteId, NoteRecipient, NoteTag, NoteType},
    transaction::OutputNote,
};
use miden_swapp::PswapNote;
use miden_tx::auth::TransactionAuthenticator;

use crate::{
    Client, ClientError,
    store::{InputNoteRecord, NoteFilter},
};

use super::{TransactionId, request::TransactionRequestBuilder};

struct MatchingPswapNote {
    record: InputNoteRecord,
    offered_amount: u64,
    requested_amount: u64,
}

struct ConsumeEntry {
    note: Note,
    note_args: Word,
    expected_future_notes: Vec<(NoteDetails, NoteTag)>,
    expected_recipients: Vec<NoteRecipient>,
}

impl<AUTH> Client<AUTH>
where
    AUTH: TransactionAuthenticator + Sync + 'static,
{
    /// Discovers matching pswap notes on chain, builds a [`TransactionRequest`] that consumes
    /// them (sorted by best exchange rate), and optionally includes a remainder pswap note if
    /// the requested asset cannot be fully obtained from existing notes.
    ///
    /// This method does **not** submit the transaction — use [`submit_pswap`](Self::submit_pswap)
    /// for a full build-and-submit flow, or pass the returned request to a wallet adapter for
    /// external signing.
    ///
    /// # Arguments
    ///
    /// * `account_id` - The account executing the swap.
    /// * `offered_asset` - The fungible asset being offered (what the caller is willing to spend).
    /// * `requested_asset` - The fungible asset being requested (what the caller wants to receive).
    /// * `pswap_note_type` - The [`NoteType`] for any newly created pswap note.
    ///
    /// # Returns
    ///
    /// Returns `(TransactionRequest, Option<NoteId>)`:
    /// - `TransactionRequest` is the assembled transaction ready for execution.
    /// - `Option<NoteId>` is `None` if fully fulfilled, or `Some(note_id)` of the newly
    ///   created pswap note if the swap was only partially fulfilled or no matching notes
    ///   were found.
    ///
    /// # Steps
    ///
    /// 1. Builds a `pswap` tag to track notes that offer the requested asset and request
    ///    the offered asset.
    /// 2. Adds the tag to the client and syncs state with the network.
    /// 3. Retrieves committed notes from the local store and filters for pswap notes that
    ///    exactly match the asset pair (filtering out tag collisions and own notes).
    /// 4. Sorts matching notes by exchange rate (best rate first — most requested asset
    ///    received per unit of offered asset spent).
    /// 5. Consumes notes greedily until the requested asset amount is fulfilled or the
    ///    offered asset budget is exhausted.
    /// 6. If there is remaining unfulfilled demand and remaining offered asset, creates a
    ///    new pswap note for the remainder.
    pub async fn build_pswap_request(
        &mut self,
        account_id: AccountId,
        offered_asset: FungibleAsset,
        requested_asset: FungibleAsset,
        pswap_note_type: NoteType,
    ) -> Result<(super::TransactionRequest, Option<NoteId>), ClientError> {
        // 1. Discover matching pswap notes on chain.
        let mut matching_notes = self
            .find_matching_pswap_notes(account_id, &offered_asset, &requested_asset)
            .await?;

        // 2. Sort by best exchange rate (most B received per A spent).
        sort_by_best_rate(&mut matching_notes);

        // 3. Build consume entries and compute remaining amounts.
        let (consume_entries, remaining_offered, remaining_requested) = build_consume_entries(
            account_id,
            &matching_notes,
            offered_asset.amount(),
            requested_asset.amount(),
        )?;

        // 4. Assemble the transaction request.
        self.build_swap_transaction(
            account_id,
            &offered_asset,
            &requested_asset,
            pswap_note_type,
            consume_entries,
            remaining_offered,
            remaining_requested,
        )
    }

    /// Convenience method that calls [`build_pswap_request`](Self::build_pswap_request) and
    /// submits the resulting transaction to the network.
    ///
    /// # Returns
    ///
    /// Returns `(TransactionId, Option<NoteId>)`:
    /// - `TransactionId` is the ID of the submitted transaction.
    /// - `Option<NoteId>` is `None` if fully fulfilled, or `Some(note_id)` of the newly
    ///   created pswap note.
    pub async fn submit_pswap(
        &mut self,
        account_id: AccountId,
        offered_asset: FungibleAsset,
        requested_asset: FungibleAsset,
        pswap_note_type: NoteType,
    ) -> Result<(TransactionId, Option<NoteId>), ClientError> {
        let (tx_request, new_pswap_note_id) = self
            .build_pswap_request(account_id, offered_asset, requested_asset, pswap_note_type)
            .await?;

        let tx_id = self.submit_new_transaction(account_id, tx_request).await?;

        Ok((tx_id, new_pswap_note_id))
    }

    // PRIVATE HELPERS
    // --------------------------------------------------------------------------------------------

    /// Tracks the pswap tag, syncs state, and returns committed pswap notes that match the
    /// desired asset pair — filtering out tag collisions and notes created by `account_id`.
    async fn find_matching_pswap_notes(
        &mut self,
        account_id: AccountId,
        offered_asset: &FungibleAsset,
        requested_asset: &FungibleAsset,
    ) -> Result<Vec<MatchingPswapNote>, ClientError> {
        // Inverted tag: search for notes that OFFER requested asset and REQUEST offered asset.
        let offered_as_asset: Asset = (*offered_asset).into();
        let requested_as_asset: Asset = (*requested_asset).into();
        let search_tag =
            PswapNote::build_tag(NoteType::Public, &requested_as_asset, &offered_as_asset);
        self.add_note_tag(search_tag).await?;
        self.sync_state().await?;

        // Retrieve committed notes and filter for matching pswap notes.
        let committed_notes = self.get_input_notes(NoteFilter::Committed).await?;
        let pswap_script_root = PswapNote::script_root();

        let mut matching = Vec::new();

        for record in committed_notes {
            if let Some(entry) = try_match_pswap_note(
                &record,
                &pswap_script_root,
                account_id,
                offered_asset,
                requested_asset,
            ) {
                matching.push(entry);
            }
        }

        Ok(matching)
    }

    /// Assembles a [`TransactionRequest`] from the consume entries and an optional new pswap
    /// note for any remaining unfulfilled amount.
    fn build_swap_transaction(
        &mut self,
        account_id: AccountId,
        offered_asset: &FungibleAsset,
        requested_asset: &FungibleAsset,
        pswap_note_type: NoteType,
        consume_entries: Vec<ConsumeEntry>,
        remaining_offered: u64,
        remaining_requested: u64,
    ) -> Result<(super::TransactionRequest, Option<NoteId>), ClientError> {
        let mut builder = TransactionRequestBuilder::new();

        // Add consumed pswap notes as input notes with their args.
        let has_consumes = !consume_entries.is_empty();
        if has_consumes {
            let mut input_notes = Vec::with_capacity(consume_entries.len());
            let mut all_future_notes = Vec::new();
            let mut all_recipients = Vec::new();

            for entry in consume_entries {
                input_notes.push((entry.note, Some(entry.note_args)));
                all_future_notes.extend(entry.expected_future_notes);
                all_recipients.extend(entry.expected_recipients);
            }

            builder = builder
                .input_notes(input_notes)
                .expected_future_notes(all_future_notes)
                .expected_output_recipients(all_recipients);
        }

        // Create a new pswap note if the swap wasn't fully fulfilled.
        let new_pswap_note_id = if remaining_requested > 0 && remaining_offered > 0 {
            // SAFETY: amounts are > 0 (guarded above), at most the original valid amounts
            // (only subtracted from), and faucet IDs come from the caller's validated assets.
            let remaining_offered_asset = Asset::Fungible(
                // unwrap: 0 < remaining_offered <= original valid amount.
                FungibleAsset::new(offered_asset.faucet_id(), remaining_offered).unwrap(),
            );
            let remaining_requested_asset = Asset::Fungible(
                // unwrap: 0 < remaining_requested <= original valid amount.
                FungibleAsset::new(requested_asset.faucet_id(), remaining_requested).unwrap(),
            );
            let note = PswapNote::create(
                account_id,
                remaining_offered_asset,
                remaining_requested_asset,
                pswap_note_type,
                NoteAttachment::default(),
                self.rng(),
            )
            .map_err(ClientError::NoteError)?;
            let id = note.id();
            builder = builder.own_output_notes(vec![OutputNote::Full(note)]);
            Some(id)
        } else if !has_consumes {
            // No matching notes at all — publish the entire swap as a new pswap note.
            let note = PswapNote::create(
                account_id,
                (*offered_asset).into(),
                (*requested_asset).into(),
                pswap_note_type,
                NoteAttachment::default(),
                self.rng(),
            )
            .map_err(ClientError::NoteError)?;
            let id = note.id();
            builder = builder.own_output_notes(vec![OutputNote::Full(note)]);
            Some(id)
        } else {
            None
        };

        let tx_request = builder.build().map_err(ClientError::TransactionRequestError)?;
        Ok((tx_request, new_pswap_note_id))
    }
}

/// Attempts to match an [`InputNoteRecord`] as a pswap note for the desired asset pair.
///
/// Returns `Some(MatchingPswapNote)` if the note:
/// - Has the pswap script root.
/// - Offers an asset matching `requested` (what the caller wants).
/// - Requests an asset matching `offered` (what the caller provides).
/// - Was not created by `account_id`.
fn try_match_pswap_note(
    record: &InputNoteRecord,
    pswap_script_root: &Word,
    account_id: AccountId,
    offered: &FungibleAsset,
    requested: &FungibleAsset,
) -> Option<MatchingPswapNote> {
    // Verify script root.
    if record.details().recipient().script().root() != *pswap_script_root {
        return None;
    }

    // Parse the note's requested asset (what the note wants is what is offered).
    let inputs = record.details().recipient().inputs().values();
    let note_requested_asset = PswapNote::get_requested_asset(inputs).ok()?;
    let (note_req_faucet, note_req_amount) = match &note_requested_asset {
        Asset::Fungible(fa) => (fa.faucet_id(), fa.amount()),
        _ => return None,
    };
    if note_req_faucet != offered.faucet_id() {
        return None;
    }

    // Parse the note's offered asset (what the note offers is what is requested).
    let note_offered_asset = record.assets().iter().next()?;
    let (note_off_faucet, note_off_amount) = match &note_offered_asset {
        Asset::Fungible(fa) => (fa.faucet_id(), fa.amount()),
        _ => return None,
    };
    if note_off_faucet != requested.faucet_id() {
        return None;
    }

    // Skip notes created by executing account.
    let inputs = record.details().recipient().inputs().values();
    let creator_id = PswapNote::get_creator_account_id(inputs).ok()?;
    if creator_id == account_id {
        return None;
    }

    // Skip notes with unfavorable exchange rate.
    // The consumer expects at least `requested.amount()` of B for `offered.amount()` of A.
    // This note offers `note_off_amount` of B and requests `note_req_amount` of A.
    // The note's rate is favorable iff:
    //   note_off_amount / note_req_amount >= requested.amount() / offered.amount()
    // Rearranged to avoid floating-point:
    //   note_off_amount * offered.amount() >= note_req_amount * requested.amount()
    let note_rate_lhs = (note_off_amount as u128) * (offered.amount() as u128);
    let note_rate_rhs = (note_req_amount as u128) * (requested.amount() as u128);
    if note_rate_lhs < note_rate_rhs {
        return None;
    }

    Some(MatchingPswapNote {
        record: record.clone(),
        offered_amount: note_off_amount,
        requested_amount: note_req_amount,
    })
}

/// Sorts matching notes in descending order by exchange rate (most B per A first).
///
/// Uses integer cross-multiplication to avoid floating-point imprecision:
/// `rate_a > rate_b  ⟺  a.offered * b.requested > b.offered * a.requested`
fn sort_by_best_rate(notes: &mut [MatchingPswapNote]) {
    notes.sort_by(|a, b| {
        let lhs = (a.offered_amount as u128) * (b.requested_amount as u128);
        let rhs = (b.offered_amount as u128) * (a.requested_amount as u128);
        rhs.cmp(&lhs)
    });
}

/// Iterates over sorted matching notes and builds [`ConsumeEntry`]s, consuming greedily
/// until the requested asset is fulfilled or the offered asset budget is exhausted.
///
/// Returns `(consume_entries, remaining_offered, remaining_requested)`.
fn build_consume_entries(
    account_id: AccountId,
    matching_notes: &[MatchingPswapNote],
    mut remaining_offered: u64,
    mut remaining_requested: u64,
) -> Result<(Vec<ConsumeEntry>, u64, u64), ClientError> {
    let mut entries = Vec::new();

    for m in matching_notes {
        if remaining_requested == 0 || remaining_offered == 0 {
            break;
        }

        let fill_amount = compute_fill_amount(
            m.offered_amount,
            m.requested_amount,
            remaining_offered,
            remaining_requested,
        );

        if fill_amount == 0 {
            continue;
        }

        let actual_output =
            PswapNote::calculate_output_amount(m.offered_amount, m.requested_amount, fill_amount);

        remaining_offered -= fill_amount;
        remaining_requested = remaining_requested.saturating_sub(actual_output);

        // Convert InputNoteRecord → Note.
        let note: Note = m.record.clone().try_into()?;

        // Note args: [0, 0, inflight_amount, fill_amount] (inflight = 0 in our case).
        let note_args =
            Word::from([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(fill_amount)]);

        // Compute expected output notes from consuming this pswap note.
        let (p2id_note, remainder_note) =
            PswapNote::create_output_notes(&note, account_id, fill_amount, 0)
                .map_err(ClientError::NoteError)?;

        let mut future_notes = vec![(NoteDetails::from(&p2id_note), p2id_note.metadata().tag())];
        let mut recipients = vec![p2id_note.recipient().clone()];

        if let Some(ref remainder) = remainder_note {
            future_notes.push((NoteDetails::from(remainder), remainder.metadata().tag()));
            recipients.push(remainder.recipient().clone());
        }

        entries.push(ConsumeEntry {
            note,
            note_args,
            expected_future_notes: future_notes,
            expected_recipients: recipients,
        });
    }

    Ok((entries, remaining_offered, remaining_requested))
}

// For the "last note" scenario (where we'd receive more than needed), a precise
// fill is computed with ceiling division and a rounding safeguard.
fn compute_fill_amount(
    note_offered: u64,
    note_requested: u64,
    remaining_offered: u64,
    remaining_requested: u64,
) -> u64 {
    // Cap by our budget and the note's total request.
    let max_fill = remaining_offered.min(note_requested);

    let output_at_max = PswapNote::calculate_output_amount(note_offered, note_requested, max_fill);

    if output_at_max <= remaining_requested {
        // We need all of this (and possibly more from subsequent notes).
        max_fill
    } else {
        // We'd receive more B than needed. Compute precise fill via ceiling division:
        //   fill ≈ ⌈remaining_requested × note_requested / note_offered⌉
        let precise_fill = (remaining_requested as u128)
            .checked_mul(note_requested as u128)
            .and_then(|n| n.checked_add(note_offered as u128 - 1))
            .map(|n| n / (note_offered as u128))
            .unwrap_or(max_fill as u128);
        let mut fill = (precise_fill as u64).min(max_fill);

        // Rounding safeguard: verify we actually get at least remaining_requested.
        let check = PswapNote::calculate_output_amount(note_offered, note_requested, fill);
        if check < remaining_requested && fill < max_fill {
            fill = (fill + 1).min(max_fill);
        }

        fill
    }
}
