package engine

import (
	"errors"

	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

// Host 是 Go 宿主向 Engine 提供的平台能力。
//
// 回调由 Rust 的 Engine 线程调用，只能返回能力结果；不得在回调内调用同一个 Engine 的任何方法
// （同步请求会等待正在执行回调的同一个线程，造成环形等待）。失败返回本包的 ErrHost* 之一，
// 其他错误按 ErrHostIO 处理。
//
// 本片只开放启动所需的目录与安全存储；剪贴板与文件句柄能力固定返回“不可用”，
// 随后续 Engine 能力一起补入。
type Host interface {
	PrivateDataDirectory() (string, error)
	CacheDirectory() (string, error)
	TemporaryDirectory() (string, error)
	// SecureStorageGet 返回 (nil, nil) 表示键不存在。
	SecureStorageGet(key string) ([]byte, error)
	SecureStorageSet(key string, value []byte) error
	SecureStorageDelete(key string) error
}

// hostAdapter 把 Host 适配为生成层的回调接口，并把 Go 错误映射为稳定的宿主错误分类。
// 宿主回调的 panic 发生在 Rust 线程调用 Go 的栈上，无法被调用方 recover；这里统一转为 ErrHostIO，
// 不携带 panic 值，避免宿主自身的缺陷终止整个进程。
type hostAdapter struct {
	host Host
}

func guard(err *error) {
	if recover() != nil {
		*err = ffi.NewHostBindingErrorIo()
	}
}

func hostError(err error) error {
	switch {
	case err == nil:
		return nil
	case errors.Is(err, ErrHostUnavailable):
		return ffi.NewHostBindingErrorUnavailable()
	case errors.Is(err, ErrHostPermissionDenied):
		return ffi.NewHostBindingErrorPermissionDenied()
	case errors.Is(err, ErrHostInvalidHandle):
		return ffi.NewHostBindingErrorInvalidHandle()
	default:
		return ffi.NewHostBindingErrorIo()
	}
}

func (a hostAdapter) PrivateDataDirectory() (value string, err error) {
	defer guard(&err)
	value, err = a.host.PrivateDataDirectory()
	return value, hostError(err)
}

func (a hostAdapter) CacheDirectory() (value string, err error) {
	defer guard(&err)
	value, err = a.host.CacheDirectory()
	return value, hostError(err)
}

func (a hostAdapter) TemporaryDirectory() (value string, err error) {
	defer guard(&err)
	value, err = a.host.TemporaryDirectory()
	return value, hostError(err)
}

func (a hostAdapter) SecureStorageGet(key string) (result *[]byte, err error) {
	defer guard(&err)
	value, err := a.host.SecureStorageGet(key)
	if err != nil {
		return nil, hostError(err)
	}
	if value == nil {
		return nil, nil
	}
	return &value, nil
}

func (a hostAdapter) SecureStorageSet(key string, value []byte) (err error) {
	defer guard(&err)
	return hostError(a.host.SecureStorageSet(key, value))
}

func (a hostAdapter) SecureStorageDelete(key string) (err error) {
	defer guard(&err)
	return hostError(a.host.SecureStorageDelete(key))
}

func (hostAdapter) FileMetadata(string) (ffi.BindingFileMetadata, error) {
	return ffi.BindingFileMetadata{}, ffi.NewHostBindingErrorUnavailable()
}

func (hostAdapter) FileReadChunk(string, uint64, uint32) ([]byte, error) {
	return nil, ffi.NewHostBindingErrorUnavailable()
}

func (hostAdapter) FileWriteChunk(string, uint64, []byte) error {
	return ffi.NewHostBindingErrorUnavailable()
}

func (hostAdapter) FileFinishWrite(string) error {
	return ffi.NewHostBindingErrorUnavailable()
}

func (hostAdapter) ClipboardRead() (ffi.BindingClipboardSnapshot, error) {
	return ffi.BindingClipboardSnapshot{}, ffi.NewHostBindingErrorUnavailable()
}

func (hostAdapter) ClipboardWrite(ffi.BindingClipboardSnapshot) error {
	return ffi.NewHostBindingErrorUnavailable()
}
