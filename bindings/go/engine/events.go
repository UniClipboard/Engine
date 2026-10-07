package engine

import (
	"fmt"
	"strings"

	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

// State 是 Engine 的生命周期状态。
type State int

const (
	StateRunning State = iota + 1
	StateQuiescing
	StateQuiesced
	StateSuspended
	StateShuttingDown
	StateStopped
)

// RefreshReason 说明为什么消费者必须重新查询权威状态。
type RefreshReason int

const (
	// RefreshConsumerLagged 表示事件队列溢出，中间事件已被丢弃。
	RefreshConsumerLagged RefreshReason = iota + 1
	RefreshStateInvalidated
)

// Event 是 Engine 事件。本片只解释生命周期与刷新事件；其余事件只报告种类（Unmapped），
// 不携带载荷，因为载荷可能含剪贴板预览、设备名或文件名。
type Event interface{ isEvent() }

// StateChanged 表示生命周期状态变化。
type StateChanged struct{ State State }

// RefreshRequired 要求消费者重新查询权威状态。
type RefreshRequired struct{ Reason RefreshReason }

// Fatal 表示 Engine 遇到不可恢复错误。
type Fatal struct{ Failure *EngineError }

// Unmapped 表示一个本片尚未映射的事件；Kind 是固定的事件种类名。
type Unmapped struct{ Kind string }

func (StateChanged) isEvent()    {}
func (RefreshRequired) isEvent() {}
func (Fatal) isEvent()           {}
func (Unmapped) isEvent()        {}

func stateFrom(state ffi.BindingEngineState) State {
	switch state {
	case ffi.BindingEngineStateRunning:
		return StateRunning
	case ffi.BindingEngineStateQuiescing:
		return StateQuiescing
	case ffi.BindingEngineStateQuiesced:
		return StateQuiesced
	case ffi.BindingEngineStateSuspended:
		return StateSuspended
	case ffi.BindingEngineStateShuttingDown:
		return StateShuttingDown
	default:
		return StateStopped
	}
}

func eventFrom(event ffi.BindingEvent) Event {
	switch value := event.(type) {
	case ffi.BindingEventStateChanged:
		return StateChanged{State: stateFrom(value.State)}
	case ffi.BindingEventRefreshRequired:
		reason := RefreshStateInvalidated
		if value.Reason == ffi.BindingRefreshReasonConsumerLagged {
			reason = RefreshConsumerLagged
		}
		return RefreshRequired{Reason: reason}
	case ffi.BindingEventFatal:
		return Fatal{Failure: &EngineError{
			Code:      value.Failure.Code,
			Category:  categoryFrom(value.Failure.Category),
			Retryable: value.Failure.Retryable,
		}}
	default:
		return Unmapped{Kind: eventKind(event)}
	}
}

// eventKind 返回生成层变体名（如 IncomingEntry），只读类型名，不触碰载荷。
func eventKind(event ffi.BindingEvent) string {
	name := fmt.Sprintf("%T", event)
	if index := strings.LastIndex(name, "BindingEvent"); index >= 0 {
		return name[index+len("BindingEvent"):]
	}
	return name
}
