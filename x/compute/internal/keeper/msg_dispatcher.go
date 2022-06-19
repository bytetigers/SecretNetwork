package keeper

import (
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"
	sdkerrors "github.com/cosmos/cosmos-sdk/types/errors"
	wasmTypes "github.com/enigmampc/SecretNetwork/go-cosmwasm/types"
	v010wasmTypes "github.com/enigmampc/SecretNetwork/go-cosmwasm/types/v010"
	v1wasmTypes "github.com/enigmampc/SecretNetwork/go-cosmwasm/types/v1"
	"github.com/enigmampc/SecretNetwork/x/compute/internal/types"
	abci "github.com/tendermint/tendermint/abci/types"
)

// Messenger is an extension point for custom wasmd message handling

type Messenger interface {
	// DispatchMsg encodes the wasmVM message and dispatches it.
	DispatchMsg(ctx sdk.Context, contractAddr sdk.AccAddress, contractIBCPortID string, msg v1wasmTypes.CosmosMsg, ogMessageVersion wasmTypes.CosmosMsgVersion) (events []sdk.Event, data [][]byte, err error)
}

// Replyer is a subset of keeper that can handle replies to submessages
type Replyer interface {
	reply(ctx sdk.Context, contractAddress sdk.AccAddress, reply v1wasmTypes.Reply, ogTx []byte, ogSigInfo wasmTypes.VerificationInfo) ([]byte, error)
}

// MessageDispatcher coordinates message sending and submessage reply/ state commits
type MessageDispatcher struct {
	messenger Messenger
	keeper    Replyer
}

// NewMessageDispatcher constructor
func NewMessageDispatcher(messenger Messenger, keeper Replyer) *MessageDispatcher {
	return &MessageDispatcher{messenger: messenger, keeper: keeper}
}

func filterEvents(events []sdk.Event) []sdk.Event {
	// pre-allocate space for efficiency
	res := make([]sdk.Event, 0, len(events))
	for _, ev := range events {
		if ev.Type != "message" {
			res = append(res, ev)
		}
	}
	return res
}

func sdkAttributesToWasmVMAttributes(attrs []abci.EventAttribute) []v010wasmTypes.LogAttribute {
	res := make([]v010wasmTypes.LogAttribute, len(attrs))
	for i, attr := range attrs {
		res[i] = v010wasmTypes.LogAttribute{
			Key:   string(attr.Key),
			Value: string(attr.Value),
		}
	}
	return res
}

func sdkEventsToWasmVMEvents(events []sdk.Event) []v1wasmTypes.Event {
	res := make([]v1wasmTypes.Event, len(events))
	for i, ev := range events {
		res[i] = v1wasmTypes.Event{
			Type:       ev.Type,
			Attributes: sdkAttributesToWasmVMAttributes(ev.Attributes),
		}
	}
	return res
}

// dispatchMsgWithGasLimit sends a message with gas limit applied
func (d MessageDispatcher) dispatchMsgWithGasLimit(ctx sdk.Context, contractAddr sdk.AccAddress, ibcPort string, msg v1wasmTypes.CosmosMsg, gasLimit uint64, ogCosmosMessageVersion wasmTypes.CosmosMsgVersion) (events []sdk.Event, data [][]byte, err error) {
	limitedMeter := sdk.NewGasMeter(gasLimit)
	subCtx := ctx.WithGasMeter(limitedMeter)

	// catch out of gas panic and just charge the entire gas limit
	defer func() {
		if r := recover(); r != nil {
			// if it's not an OutOfGas error, raise it again
			if _, ok := r.(sdk.ErrorOutOfGas); !ok {
				// log it to get the original stack trace somewhere (as panic(r) keeps message but stacktrace to here
				moduleLogger(ctx).Info("SubMsg rethrowing panic: %#v", r)
				panic(r)
			}
			ctx.GasMeter().ConsumeGas(gasLimit, "Sub-Message OutOfGas panic")
			err = sdkerrors.Wrap(sdkerrors.ErrOutOfGas, "SubMsg hit gas limit")
		}
	}()
	events, data, err = d.messenger.DispatchMsg(subCtx, contractAddr, ibcPort, msg, ogCosmosMessageVersion)

	// make sure we charge the parent what was spent
	spent := subCtx.GasMeter().GasConsumed()
	ctx.GasMeter().ConsumeGas(spent, "From limited Sub-Message")

	return events, data, err
}

type InvalidRequest struct {
	Err     string `json:"error"`
	Request []byte `json:"request"`
}

func (e InvalidRequest) Error() string {
	return fmt.Sprintf("invalid request: %s - original request: %s", e.Err, string(e.Request))
}

type InvalidResponse struct {
	Err      string `json:"error"`
	Response []byte `json:"response"`
}

func (e InvalidResponse) Error() string {
	return fmt.Sprintf("invalid response: %s - original response: %s", e.Err, string(e.Response))
}

type NoSuchContract struct {
	Addr string `json:"addr,omitempty"`
}

func (e NoSuchContract) Error() string {
	return fmt.Sprintf("no such contract: %s", e.Addr)
}

type Unknown struct{}

func (e Unknown) Error() string {
	return "unknown system error"
}

type UnsupportedRequest struct {
	Kind string `json:"kind,omitempty"`
}

func (e UnsupportedRequest) Error() string {
	return fmt.Sprintf("unsupported request: %s", e.Kind)
}

// Reply is encrypted on when it is a contract reply and it is OK since error is always reducted to be a string.
func isReplyEncrypted(msg v1wasmTypes.CosmosMsg, reply v1wasmTypes.Reply) bool {
	return (msg.Wasm != nil) && (reply.Result.Ok != nil)
}

// Issue #759 - we don't return error string for worries of non-determinism
func redactError(err error) error {
	// Do not redact system errors
	// SystemErrors must be created in x/wasm and we can ensure determinism
	if wasmTypes.ToSystemError(err) != nil {
		return err
	}

	// FIXME: do we want to hardcode some constant string mappings here as well?
	// Or better document them? (SDK error string may change on a patch release to fix wording)
	// sdk/11 is out of gas
	// sdk/5 is insufficient funds (on bank send)
	// (we can theoretically redact less in the future, but this is a first step to safety)
	codespace, code, _ := sdkerrors.ABCIInfo(err, false)
	return fmt.Errorf("codespace: %s, code: %d", codespace, code)
}

// DispatchSubmessages builds a sandbox to execute these messages and returns the execution result to the contract
// that dispatched them, both on success as well as failure
func (d MessageDispatcher) DispatchSubmessages(ctx sdk.Context, contractAddr sdk.AccAddress, ibcPort string, msgs []v1wasmTypes.SubMsg, ogTx []byte, ogSigInfo wasmTypes.VerificationInfo, ogCosmosMessageVersion wasmTypes.CosmosMsgVersion) ([]byte, error) {
	var rsp []byte
	for _, msg := range msgs {
		// Check replyOn validity
		switch msg.ReplyOn {
		case v1wasmTypes.ReplySuccess, v1wasmTypes.ReplyError, v1wasmTypes.ReplyAlways, v1wasmTypes.ReplyNever:
		default:
			return nil, sdkerrors.Wrap(types.ErrInvalid, "replyOn value")
		}

		// first, we build a sub-context which we can use inside the submessages
		subCtx, commit := ctx.CacheContext()
		em := sdk.NewEventManager()
		subCtx = subCtx.WithEventManager(em)

		// check how much gas left locally, optionally wrap the gas meter
		gasRemaining := ctx.GasMeter().Limit() - ctx.GasMeter().GasConsumed()
		limitGas := msg.GasLimit != nil && (*msg.GasLimit < gasRemaining)

		var err error
		var events []sdk.Event
		var data [][]byte
		if limitGas {
			events, data, err = d.dispatchMsgWithGasLimit(subCtx, contractAddr, ibcPort, msg.Msg, *msg.GasLimit, ogCosmosMessageVersion)
		} else {
			events, data, err = d.messenger.DispatchMsg(subCtx, contractAddr, ibcPort, msg.Msg, ogCosmosMessageVersion)
		}

		// if it succeeds, commit state changes from submessage, and pass on events to Event Manager
		var filteredEvents []sdk.Event
		if err == nil {
			commit()
			filteredEvents = filterEvents(append(em.Events(), events...))
			ctx.EventManager().EmitEvents(filteredEvents)
		} // on failure, revert state from sandbox, and ignore events (just skip doing the above)

		// we only callback if requested. Short-circuit here the cases we don't want to
		if (msg.ReplyOn == v1wasmTypes.ReplySuccess || msg.ReplyOn == v1wasmTypes.ReplyNever) && err != nil {
			// Note: this also handles the case of v010 submessage for which the execution failed
			return nil, err
		}

		if msg.ReplyOn == v1wasmTypes.ReplyNever || (msg.ReplyOn == v1wasmTypes.ReplyError && err == nil) {
			continue
		}

		// If we are here it means that ReplySuccess and success OR ReplyError and there were errors OR ReplyAlways.
		// Basically, handle replying to the contract
		// We need to create a SubMsgResult and pass it into the calling contract
		var result v1wasmTypes.SubMsgResult
		if err == nil {
			// just take the first one for now if there are multiple sub-sdk messages
			// and safely return nothing if no data
			var responseData []byte
			if len(data) > 0 {
				responseData = data[0]
			}
			result = v1wasmTypes.SubMsgResult{
				// Copy first 64 bytes of the OG message in order to preserve the pubkey.
				Ok: &v1wasmTypes.SubMsgResponse{
					Events: sdkEventsToWasmVMEvents(filteredEvents),
					Data:   responseData,
				},
			}
		} else {
			// Issue #759 - we don't return error string for worries of non-determinism
			moduleLogger(ctx).Info("Redacting submessage error", "cause", err)
			result = v1wasmTypes.SubMsgResult{
				Err: redactError(err).Error(),
			}
		}

		// now handle the reply, we use the parent context, and abort on error
		reply := v1wasmTypes.Reply{
			ID:     msg.ID,
			Result: result,
		}

		// we can ignore any result returned as there is nothing to do with the data
		// and the events are already in the ctx.EventManager()

		// In order to specify that the reply isn't signed by the enclave we use "SIGN_MODE_UNSPECIFIED"
		// The SGX will notice that the value is SIGN_MODE_UNSPECIFIED and will treat the message as plaintext.
		replySigInfo := wasmTypes.VerificationInfo{
			Bytes:     []byte{},
			ModeInfo:  []byte{},
			PublicKey: []byte{},
			Signature: []byte{},
			SignMode:  "SIGN_MODE_UNSPECIFIED",
		}
		if isReplyEncrypted(msg.Msg, reply) {
			replySigInfo = ogSigInfo
		}

		rspData, err := d.keeper.reply(ctx, contractAddr, reply, ogTx, replySigInfo)
		switch {
		case err != nil:
			return nil, err
		case rspData != nil:
			rsp = rspData
		}
	}
	return rsp, nil
}
