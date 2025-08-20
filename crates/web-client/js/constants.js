export const WorkerAction = Object.freeze({
  INIT: "init",
  CALL_METHOD: "callMethod",
});

export const MethodName = Object.freeze({
  CREATE_CLIENT: "createClient",
  NEW_WALLET: "newWallet",
  NEW_FAUCET: "newFaucet",
  NEW_TRANSACTION: "newTransaction",
  NEW_MULTISIG_TRANSACTION: "newMultisigTransaction",
  PROPOSE_MULTISIG_TRANSACTION: "proposeMultisigTransaction",
  SUBMIT_TRANSACTION: "submitTransaction",
  SYNC_STATE: "syncState",
});
