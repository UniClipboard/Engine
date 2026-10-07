package engine

import (
	"errors"
	"fmt"

	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

var (
	// ErrClosed 表示 Engine 已关闭或正在关闭；不会进入 Rust。
	ErrClosed = errors.New("engine is closed")
	// ErrInvalidInput 表示调用方输入在进入 Rust 之前就被拒绝。
	ErrInvalidInput = errors.New("invalid input")
	// ErrCloseIncomplete 表示 Close 的期限先耗尽：关闭已请求，但 Rust 关闭或在途调用尚未结束。
	// 引擎不会被标记为已停止，可以用新的期限再次调用 Close。
	ErrCloseIncomplete = errors.New("engine close did not complete within its deadline")
	// ErrRuntimeUnavailable 与 ErrAlreadyStopped 对应 Rust 绑定的同名稳定错误。
	ErrRuntimeUnavailable = errors.New("engine runtime unavailable")
	ErrAlreadyStopped     = errors.New("engine already stopped")
	// ErrUnexpectedResult 表示 Rust 返回了绑定无法解释的结果。
	ErrUnexpectedResult = errors.New("engine returned an unexpected result")
)

// 宿主能力错误：Host 实现返回这些值之一，Rust 侧据此得到稳定的宿主错误分类。
var (
	ErrHostUnavailable      = errors.New("host capability unavailable")
	ErrHostPermissionDenied = errors.New("host capability permission denied")
	ErrHostInvalidHandle    = errors.New("host file handle invalid")
	ErrHostIO               = errors.New("host input/output failed")
)

// ErrorCategory 是 Engine 稳定错误的粗分类。
type ErrorCategory int

const (
	CategoryInvalidInput ErrorCategory = iota + 1
	CategoryInvalidState
	CategoryUnauthorized
	CategoryNotFound
	CategoryConflict
	CategoryUnavailable
	CategoryDeadlineExceeded
	CategoryInternal
)

// EngineError 是 Engine 业务失败的稳定形态：只有错误码、分类与可重试性，不含任何内容、路径或设备信息。
type EngineError struct {
	Code      uint32
	Category  ErrorCategory
	Retryable bool
}

func (e *EngineError) Error() string {
	return fmt.Sprintf("engine error %d (category %d)", e.Code, e.Category)
}

// convertError 把生成层错误归一为本包的稳定错误；未识别的错误按 ErrUnexpectedResult 处理。
func convertError(err error) error {
	if err == nil {
		return nil
	}
	var binding *ffi.BindingError
	if !errors.As(err, &binding) {
		var value ffi.BindingError
		if !errors.As(err, &value) {
			return err
		}
		binding = &value
	}
	var engine *ffi.BindingErrorEngine
	switch {
	case errors.As(binding, &engine):
		return &EngineError{
			Code:      engine.Code,
			Category:  categoryFrom(engine.Category),
			Retryable: engine.Retryable,
		}
	case errors.Is(binding, ffi.ErrBindingErrorHostUnavailable):
		return ErrHostUnavailable
	case errors.Is(binding, ffi.ErrBindingErrorHostPermissionDenied):
		return ErrHostPermissionDenied
	case errors.Is(binding, ffi.ErrBindingErrorHostInvalidHandle):
		return ErrHostInvalidHandle
	case errors.Is(binding, ffi.ErrBindingErrorHostIo):
		return ErrHostIO
	case errors.Is(binding, ffi.ErrBindingErrorRuntimeUnavailable):
		return ErrRuntimeUnavailable
	case errors.Is(binding, ffi.ErrBindingErrorAlreadyStopped):
		return ErrAlreadyStopped
	default:
		return ErrUnexpectedResult
	}
}

func categoryFrom(category ffi.BindingErrorCategory) ErrorCategory {
	switch category {
	case ffi.BindingErrorCategoryInvalidInput:
		return CategoryInvalidInput
	case ffi.BindingErrorCategoryInvalidState:
		return CategoryInvalidState
	case ffi.BindingErrorCategoryUnauthorized:
		return CategoryUnauthorized
	case ffi.BindingErrorCategoryNotFound:
		return CategoryNotFound
	case ffi.BindingErrorCategoryConflict:
		return CategoryConflict
	case ffi.BindingErrorCategoryUnavailable:
		return CategoryUnavailable
	case ffi.BindingErrorCategoryDeadlineExceeded:
		return CategoryDeadlineExceeded
	default:
		return CategoryInternal
	}
}
