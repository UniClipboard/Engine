package uc_engine_uniffi

// #include <uc_engine_uniffi.h>
import "C"

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"runtime"
	"sync"
	"sync/atomic"
	"unsafe"
)

// This is needed, because as of go 1.24
// type RustBuffer C.RustBuffer cannot have methods,
// RustBuffer is treated as non-local type
type GoRustBuffer struct {
	inner C.RustBuffer
}

type RustBufferI interface {
	AsReader() *bytes.Reader
	Free()
	ToGoBytes() []byte
	Data() unsafe.Pointer
	Len() uint64
	Capacity() uint64
}

// C.RustBuffer fields exposed as an interface so they can be accessed in different Go packages.
// See https://github.com/golang/go/issues/13467
type ExternalCRustBuffer interface {
	Data() unsafe.Pointer
	Len() uint64
	Capacity() uint64
}

func RustBufferFromC(b C.RustBuffer) ExternalCRustBuffer {
	return GoRustBuffer{
		inner: b,
	}
}

func CFromRustBuffer(b ExternalCRustBuffer) C.RustBuffer {
	return C.RustBuffer{
		capacity: C.uint64_t(b.Capacity()),
		len:      C.uint64_t(b.Len()),
		data:     (*C.uchar)(b.Data()),
	}
}

func RustBufferFromExternal(b ExternalCRustBuffer) GoRustBuffer {
	return GoRustBuffer{
		inner: C.RustBuffer{
			capacity: C.uint64_t(b.Capacity()),
			len:      C.uint64_t(b.Len()),
			data:     (*C.uchar)(b.Data()),
		},
	}
}

func (cb GoRustBuffer) Capacity() uint64 {
	return uint64(cb.inner.capacity)
}

func (cb GoRustBuffer) Len() uint64 {
	return uint64(cb.inner.len)
}

func (cb GoRustBuffer) Data() unsafe.Pointer {
	return unsafe.Pointer(cb.inner.data)
}

func (cb GoRustBuffer) AsReader() *bytes.Reader {
	b := unsafe.Slice((*byte)(cb.inner.data), C.uint64_t(cb.inner.len))
	return bytes.NewReader(b)
}

func (cb GoRustBuffer) Free() {
	rustCall(func(status *C.RustCallStatus) bool {
		C.ffi_uc_engine_uniffi_rustbuffer_free(cb.inner, status)
		return false
	})
}

func (cb GoRustBuffer) ToGoBytes() []byte {
	return C.GoBytes(unsafe.Pointer(cb.inner.data), C.int(cb.inner.len))
}

func stringToRustBuffer(str string) C.RustBuffer {
	return bytesToRustBuffer([]byte(str))
}

func bytesToRustBuffer(b []byte) C.RustBuffer {
	if len(b) == 0 {
		return C.RustBuffer{}
	}
	// We can pass the pointer along here, as it is pinned
	// for the duration of this call
	foreign := C.ForeignBytes{
		len:  C.int(len(b)),
		data: (*C.uchar)(unsafe.Pointer(&b[0])),
	}

	return rustCall(func(status *C.RustCallStatus) C.RustBuffer {
		return C.ffi_uc_engine_uniffi_rustbuffer_from_bytes(foreign, status)
	})
}

type BufLifter[GoType any] interface {
	Lift(value RustBufferI) GoType
}

type BufLowerer[GoType any] interface {
	Lower(value GoType) C.RustBuffer
}

type BufReader[GoType any] interface {
	Read(reader io.Reader) GoType
}

type BufWriter[GoType any] interface {
	Write(writer io.Writer, value GoType)
}

func LowerIntoRustBuffer[GoType any](bufWriter BufWriter[GoType], value GoType) C.RustBuffer {
	// This might be not the most efficient way but it does not require knowing allocation size
	// beforehand
	var buffer bytes.Buffer
	bufWriter.Write(&buffer, value)

	bytes, err := io.ReadAll(&buffer)
	if err != nil {
		panic(fmt.Errorf("reading written data: %w", err))
	}
	return bytesToRustBuffer(bytes)
}

func LiftFromRustBuffer[GoType any](bufReader BufReader[GoType], rbuf RustBufferI) GoType {
	defer rbuf.Free()
	reader := rbuf.AsReader()
	item := bufReader.Read(reader)
	if reader.Len() > 0 {
		// TODO: Remove this
		leftover, _ := io.ReadAll(reader)
		panic(fmt.Errorf("Junk remaining in buffer after lifting: %s", string(leftover)))
	}
	return item
}

func rustCallWithError[E any, U any](converter BufReader[E], callback func(*C.RustCallStatus) U) (U, E) {
	var status C.RustCallStatus
	returnValue := callback(&status)
	err := checkCallStatus(converter, status)
	return returnValue, err
}

func checkCallStatus[E any](converter BufReader[E], status C.RustCallStatus) E {
	switch status.code {
	case 0:
		var zero E
		return zero
	case 1:
		return LiftFromRustBuffer(converter, GoRustBuffer{inner: status.errorBuf})
	case 2:
		// when the rust code sees a panic, it tries to construct a rustBuffer
		// with the message.  but if that code panics, then it just sends back
		// an empty buffer.
		if status.errorBuf.len > 0 {
			panic(fmt.Errorf("%s", FfiConverterStringINSTANCE.Lift(GoRustBuffer{inner: status.errorBuf})))
		} else {
			panic(fmt.Errorf("Rust panicked while handling Rust panic"))
		}
	default:
		panic(fmt.Errorf("unknown status code: %d", status.code))
	}
}

func checkCallStatusUnknown(status C.RustCallStatus) error {
	switch status.code {
	case 0:
		return nil
	case 1:
		panic(fmt.Errorf("function not returning an error returned an error"))
	case 2:
		// when the rust code sees a panic, it tries to construct a C.RustBuffer
		// with the message.  but if that code panics, then it just sends back
		// an empty buffer.
		if status.errorBuf.len > 0 {
			panic(fmt.Errorf("%s", FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: status.errorBuf,
			})))
		} else {
			panic(fmt.Errorf("Rust panicked while handling Rust panic"))
		}
	default:
		return fmt.Errorf("unknown status code: %d", status.code)
	}
}

func rustCall[U any](callback func(*C.RustCallStatus) U) U {
	returnValue, err := rustCallWithError[error](nil, callback)
	if err != nil {
		panic(err)
	}
	return returnValue
}

type NativeError interface {
	AsError() error
}

func writeInt8(writer io.Writer, value int8) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint8(writer io.Writer, value uint8) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt16(writer io.Writer, value int16) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint16(writer io.Writer, value uint16) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt32(writer io.Writer, value int32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint32(writer io.Writer, value uint32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt64(writer io.Writer, value int64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint64(writer io.Writer, value uint64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeFloat32(writer io.Writer, value float32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeFloat64(writer io.Writer, value float64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func readInt8(reader io.Reader) int8 {
	var result int8
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint8(reader io.Reader) uint8 {
	var result uint8
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt16(reader io.Reader) int16 {
	var result int16
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint16(reader io.Reader) uint16 {
	var result uint16
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt32(reader io.Reader) int32 {
	var result int32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint32(reader io.Reader) uint32 {
	var result uint32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt64(reader io.Reader) int64 {
	var result int64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint64(reader io.Reader) uint64 {
	var result uint64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readFloat32(reader io.Reader) float32 {
	var result float32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readFloat64(reader io.Reader) float64 {
	var result float64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func init() {

	FfiConverterBindingAnalyticsHostINSTANCE.register()
	FfiConverterBindingHostINSTANCE.register()
	uniffiCheckChecksums()
}

func uniffiCheckChecksums() {
	// Get the bindings contract version from our ComponentInterface
	bindingsContractVersion := 30
	// Get the scaffolding contract version by calling the into the dylib
	scaffoldingContractVersion := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint32_t {
		return C.ffi_uc_engine_uniffi_uniffi_contract_version()
	})
	if bindingsContractVersion != int(scaffoldingContractVersion) {
		// If this happens try cleaning and rebuilding your project
		panic("uc_engine_uniffi: UniFFI contract version mismatch")
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_core_version()
		})
		if checksum != 33681 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_core_version: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_flush_process_observability()
		})
		if checksum != 21070 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_flush_process_observability: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_install_process_observability()
		})
		if checksum != 47740 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_install_process_observability: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_query_process_observability_health()
		})
		if checksum != 17532 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_query_process_observability_health: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_shutdown_process_observability()
		})
		if checksum != 50929 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_shutdown_process_observability: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_prepare_local_diagnostic_export()
		})
		if checksum != 64660 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_prepare_local_diagnostic_export: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_query_local_diagnostic_status()
		})
		if checksum != 19411 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_query_local_diagnostic_status: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_record_host_diagnostic()
		})
		if checksum != 28654 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_record_host_diagnostic: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_register_host_diagnostic_source()
		})
		if checksum != 16115 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_register_host_diagnostic_source: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_start_local_diagnostic_capture()
		})
		if checksum != 58064 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_start_local_diagnostic_capture: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_func_stop_local_diagnostic_capture()
		})
		if checksum != 42842 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_func_stop_local_diagnostic_capture: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_capture()
		})
		if checksum != 46569 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_capture: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_identify()
		})
		if checksum != 31782 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_identify: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_group_identify()
		})
		if checksum != 63274 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_group_identify: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_adopt_space_person()
		})
		if checksum != 46798 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_adopt_space_person: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_release_space_person()
		})
		if checksum != 46412 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_release_space_person: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_current_space_person_id()
		})
		if checksum != 15586 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_current_space_person_id: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_reset_telemetry_identity()
		})
		if checksum != 26792 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinganalyticshost_reset_telemetry_identity: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_private_data_directory()
		})
		if checksum != 2874 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_private_data_directory: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_cache_directory()
		})
		if checksum != 2193 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_cache_directory: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_temporary_directory()
		})
		if checksum != 23970 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_temporary_directory: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_get()
		})
		if checksum != 19843 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_get: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_set()
		})
		if checksum != 45581 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_set: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_delete()
		})
		if checksum != 5374 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_secure_storage_delete: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_metadata()
		})
		if checksum != 30202 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_metadata: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_read_chunk()
		})
		if checksum != 40871 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_read_chunk: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_write_chunk()
		})
		if checksum != 15520 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_write_chunk: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_finish_write()
		})
		if checksum != 10603 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_file_finish_write: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_clipboard_read()
		})
		if checksum != 44572 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_clipboard_read: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_bindinghost_clipboard_write()
		})
		if checksum != 22129 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_bindinghost_clipboard_write: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_add_custom_relay()
		})
		if checksum != 36381 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_add_custom_relay: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_cancel_join_space()
		})
		if checksum != 49819 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_cancel_join_space: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_capture_current_clipboard()
		})
		if checksum != 59446 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_capture_current_clipboard: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_change_encryption_passphrase()
		})
		if checksum != 40324 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_change_encryption_passphrase: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_choose_device_group()
		})
		if checksum != 2594 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_choose_device_group: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_create_space()
		})
		if checksum != 21380 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_create_space: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_delete_custom_relay()
		})
		if checksum != 1733 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_delete_custom_relay: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_edit_custom_relay()
		})
		if checksum != 37096 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_edit_custom_relay: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_export_entry()
		})
		if checksum != 3505 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_export_entry: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_issue_invitation()
		})
		if checksum != 43331 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_issue_invitation: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_join_space()
		})
		if checksum != 61423 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_join_space: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_leave_space()
		})
		if checksum != 53173 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_leave_space: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_lifecycle_state()
		})
		if checksum != 54350 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_lifecycle_state: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_list_devices()
		})
		if checksum != 42635 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_list_devices: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_next_event()
		})
		if checksum != 60947 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_next_event: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_notify_connectivity_opportunity()
		})
		if checksum != 47459 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_notify_connectivity_opportunity: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_observe_clipboard_change()
		})
		if checksum != 48343 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_observe_clipboard_change: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_active_clipboard()
		})
		if checksum != 11249 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_active_clipboard: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_custom_relays()
		})
		if checksum != 17925 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_custom_relays: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_device_group_choices()
		})
		if checksum != 23271 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_device_group_choices: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_local_device()
		})
		if checksum != 6632 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_local_device: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_network_recovery_status()
		})
		if checksum != 43747 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_network_recovery_status: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_relay_overview()
		})
		if checksum != 52676 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_relay_overview: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_space_state()
		})
		if checksum != 25195 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_query_space_state: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_recover_network()
		})
		if checksum != 42154 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_recover_network: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_recover_session()
		})
		if checksum != 26112 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_recover_session: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_refresh_peer_connections()
		})
		if checksum != 32331 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_refresh_peer_connections: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_remove_member()
		})
		if checksum != 23868 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_remove_member: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_resend_entry()
		})
		if checksum != 19924 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_resend_entry: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_restore_clipboard()
		})
		if checksum != 59783 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_restore_clipboard: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_resume()
		})
		if checksum != 12890 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_resume: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_save_custom_relay()
		})
		if checksum != 38821 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_save_custom_relay: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_files()
		})
		if checksum != 35463 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_files: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_image()
		})
		if checksum != 2933 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_image: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_text()
		})
		if checksum != 44276 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_send_text: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_shutdown()
		})
		if checksum != 40531 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_shutdown: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_suspend()
		})
		if checksum != 27030 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_suspend: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobileengine_suspend_with_deadline()
		})
		if checksum != 2817 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobileengine_suspend_with_deadline: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_resume()
		})
		if checksum != 48315 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_resume: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_suspend()
		})
		if checksum != 15241 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_suspend: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_suspend_with_deadline()
		})
		if checksum != 61789 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_method_mobilestartuplifecycle_suspend_with_deadline: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start()
		})
		if checksum != 63033 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_analytics()
		})
		if checksum != 35192 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_analytics: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_analytics_and_lifecycle()
		})
		if checksum != 25026 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_analytics_and_lifecycle: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_lifecycle()
		})
		if checksum != 16941 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_constructor_mobileengine_start_with_lifecycle: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_uc_engine_uniffi_checksum_constructor_mobilestartuplifecycle_new()
		})
		if checksum != 4465 {
			// If this happens try cleaning and rebuilding your project
			panic("uc_engine_uniffi: uniffi_uc_engine_uniffi_checksum_constructor_mobilestartuplifecycle_new: UniFFI API checksum mismatch")
		}
	}
}

type FfiConverterUint32 struct{}

var FfiConverterUint32INSTANCE = FfiConverterUint32{}

func (FfiConverterUint32) Lower(value uint32) C.uint32_t {
	return C.uint32_t(value)
}

func (FfiConverterUint32) Write(writer io.Writer, value uint32) {
	writeUint32(writer, value)
}

func (FfiConverterUint32) Lift(value C.uint32_t) uint32 {
	return uint32(value)
}

func (FfiConverterUint32) Read(reader io.Reader) uint32 {
	return readUint32(reader)
}

type FfiDestroyerUint32 struct{}

func (FfiDestroyerUint32) Destroy(_ uint32) {}

type FfiConverterUint64 struct{}

var FfiConverterUint64INSTANCE = FfiConverterUint64{}

func (FfiConverterUint64) Lower(value uint64) C.uint64_t {
	return C.uint64_t(value)
}

func (FfiConverterUint64) Write(writer io.Writer, value uint64) {
	writeUint64(writer, value)
}

func (FfiConverterUint64) Lift(value C.uint64_t) uint64 {
	return uint64(value)
}

func (FfiConverterUint64) Read(reader io.Reader) uint64 {
	return readUint64(reader)
}

type FfiDestroyerUint64 struct{}

func (FfiDestroyerUint64) Destroy(_ uint64) {}

type FfiConverterInt64 struct{}

var FfiConverterInt64INSTANCE = FfiConverterInt64{}

func (FfiConverterInt64) Lower(value int64) C.int64_t {
	return C.int64_t(value)
}

func (FfiConverterInt64) Write(writer io.Writer, value int64) {
	writeInt64(writer, value)
}

func (FfiConverterInt64) Lift(value C.int64_t) int64 {
	return int64(value)
}

func (FfiConverterInt64) Read(reader io.Reader) int64 {
	return readInt64(reader)
}

type FfiDestroyerInt64 struct{}

func (FfiDestroyerInt64) Destroy(_ int64) {}

type FfiConverterBool struct{}

var FfiConverterBoolINSTANCE = FfiConverterBool{}

func (FfiConverterBool) Lower(value bool) C.int8_t {
	if value {
		return C.int8_t(1)
	}
	return C.int8_t(0)
}

func (FfiConverterBool) Write(writer io.Writer, value bool) {
	if value {
		writeInt8(writer, 1)
	} else {
		writeInt8(writer, 0)
	}
}

func (FfiConverterBool) Lift(value C.int8_t) bool {
	return value != 0
}

func (FfiConverterBool) Read(reader io.Reader) bool {
	return readInt8(reader) != 0
}

type FfiDestroyerBool struct{}

func (FfiDestroyerBool) Destroy(_ bool) {}

type FfiConverterString struct{}

var FfiConverterStringINSTANCE = FfiConverterString{}

func (FfiConverterString) Lift(rb RustBufferI) string {
	defer rb.Free()
	reader := rb.AsReader()
	b, err := io.ReadAll(reader)
	if err != nil {
		panic(fmt.Errorf("reading reader: %w", err))
	}
	return string(b)
}

func (FfiConverterString) Read(reader io.Reader) string {
	length := readInt32(reader)
	buffer := make([]byte, length)
	read_length, err := reader.Read(buffer)
	if err != nil && err != io.EOF {
		panic(err)
	}
	if read_length != int(length) {
		panic(fmt.Errorf("bad read length when reading string, expected %d, read %d", length, read_length))
	}
	return string(buffer)
}

func (FfiConverterString) Lower(value string) C.RustBuffer {
	return stringToRustBuffer(value)
}

func (c FfiConverterString) LowerExternal(value string) ExternalCRustBuffer {
	return RustBufferFromC(stringToRustBuffer(value))
}

func (FfiConverterString) Write(writer io.Writer, value string) {
	if len(value) > math.MaxInt32 {
		panic("String is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	write_length, err := io.WriteString(writer, value)
	if err != nil {
		panic(err)
	}
	if write_length != len(value) {
		panic(fmt.Errorf("bad write length when writing string, expected %d, written %d", len(value), write_length))
	}
}

type FfiDestroyerString struct{}

func (FfiDestroyerString) Destroy(_ string) {}

type FfiConverterBytes struct{}

var FfiConverterBytesINSTANCE = FfiConverterBytes{}

func (c FfiConverterBytes) Lower(value []byte) C.RustBuffer {
	return LowerIntoRustBuffer[[]byte](c, value)
}

func (c FfiConverterBytes) LowerExternal(value []byte) ExternalCRustBuffer {
	return RustBufferFromC(c.Lower(value))
}

func (c FfiConverterBytes) Write(writer io.Writer, value []byte) {
	if len(value) > math.MaxInt32 {
		panic("[]byte is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	write_length, err := writer.Write(value)
	if err != nil {
		panic(err)
	}
	if write_length != len(value) {
		panic(fmt.Errorf("bad write length when writing []byte, expected %d, written %d", len(value), write_length))
	}
}

func (c FfiConverterBytes) Lift(rb RustBufferI) []byte {
	return LiftFromRustBuffer[[]byte](c, rb)
}

func (c FfiConverterBytes) Read(reader io.Reader) []byte {
	length := readInt32(reader)
	buffer := make([]byte, length)
	read_length, err := reader.Read(buffer)
	if err != nil && err != io.EOF {
		panic(err)
	}
	if read_length != int(length) {
		panic(fmt.Errorf("bad read length when reading []byte, expected %d, read %d", length, read_length))
	}
	return buffer
}

type FfiDestroyerBytes struct{}

func (FfiDestroyerBytes) Destroy(_ []byte) {}

// Below is an implementation of synchronization requirements outlined in the link.
// https://github.com/mozilla/uniffi-rs/blob/0dc031132d9493ca812c3af6e7dd60ad2ea95bf0/uniffi_bindgen/src/bindings/kotlin/templates/ObjectRuntime.kt#L31

type FfiObject struct {
	handle        C.uint64_t
	callCounter   atomic.Int64
	cloneFunction func(C.uint64_t, *C.RustCallStatus) C.uint64_t
	freeFunction  func(C.uint64_t, *C.RustCallStatus)
	destroyed     atomic.Bool
}

func newFfiObject(
	handle C.uint64_t,
	cloneFunction func(C.uint64_t, *C.RustCallStatus) C.uint64_t,
	freeFunction func(C.uint64_t, *C.RustCallStatus),
) FfiObject {
	return FfiObject{
		handle:        handle,
		cloneFunction: cloneFunction,
		freeFunction:  freeFunction,
	}
}

func (ffiObject *FfiObject) incrementPointer(debugName string) C.uint64_t {
	for {
		counter := ffiObject.callCounter.Load()
		if counter <= -1 {
			panic(fmt.Errorf("%v object has already been destroyed", debugName))
		}
		if counter == math.MaxInt64 {
			panic(fmt.Errorf("%v object call counter would overflow", debugName))
		}
		if ffiObject.callCounter.CompareAndSwap(counter, counter+1) {
			break
		}
	}

	return rustCall(func(status *C.RustCallStatus) C.uint64_t {
		return ffiObject.cloneFunction(ffiObject.handle, status)
	})
}

func (ffiObject *FfiObject) decrementPointer() {
	if ffiObject.callCounter.Add(-1) == -1 {
		ffiObject.freeRustArcPtr()
	}
}

func (ffiObject *FfiObject) destroy() {
	if ffiObject.destroyed.CompareAndSwap(false, true) {
		if ffiObject.callCounter.Add(-1) == -1 {
			ffiObject.freeRustArcPtr()
		}
	}
}

func (ffiObject *FfiObject) freeRustArcPtr() {
	if ffiObject.handle == 0 {
		return
	}
	rustCall(func(status *C.RustCallStatus) int32 {
		ffiObject.freeFunction(ffiObject.handle, status)
		return 0
	})
}

type BindingAnalyticsHost interface {
	Capture(event BindingAnalyticsEvent) error
	Identify(payload BindingAnalyticsIdentify) error
	GroupIdentify(payload BindingAnalyticsGroupIdentify) error
	AdoptSpacePerson(spacePersonId string) (BindingAnalyticsIdentityChange, error)
	ReleaseSpacePerson() (BindingAnalyticsIdentityChange, error)
	CurrentSpacePersonId() (*string, error)
	ResetTelemetryIdentity() (BindingAnalyticsIdentityChange, error)
}
type BindingAnalyticsHostImpl struct {
	ffiObject FfiObject
}

func (_self *BindingAnalyticsHostImpl) Capture(event BindingAnalyticsEvent) error {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_capture(
			_pointer, FfiConverterBindingAnalyticsEventINSTANCE.Lower(event), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingAnalyticsHostImpl) Identify(payload BindingAnalyticsIdentify) error {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_identify(
			_pointer, FfiConverterBindingAnalyticsIdentifyINSTANCE.Lower(payload), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingAnalyticsHostImpl) GroupIdentify(payload BindingAnalyticsGroupIdentify) error {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_group_identify(
			_pointer, FfiConverterBindingAnalyticsGroupIdentifyINSTANCE.Lower(payload), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingAnalyticsHostImpl) AdoptSpacePerson(spacePersonId string) (BindingAnalyticsIdentityChange, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_adopt_space_person(
				_pointer, FfiConverterStringINSTANCE.Lower(spacePersonId), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingAnalyticsIdentityChange
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingAnalyticsHostImpl) ReleaseSpacePerson() (BindingAnalyticsIdentityChange, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_release_space_person(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingAnalyticsIdentityChange
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingAnalyticsHostImpl) CurrentSpacePersonId() (*string, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_current_space_person_id(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterOptionalStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingAnalyticsHostImpl) ResetTelemetryIdentity() (BindingAnalyticsIdentityChange, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingAnalyticsHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingAnalyticsHostError](FfiConverterBindingAnalyticsHostError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinganalyticshost_reset_telemetry_identity(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingAnalyticsIdentityChange
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lift(_uniffiRV), nil
	}
}
func (object *BindingAnalyticsHostImpl) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterBindingAnalyticsHost struct {
	handleMap *concurrentHandleMap[BindingAnalyticsHost]
}

var FfiConverterBindingAnalyticsHostINSTANCE = FfiConverterBindingAnalyticsHost{
	handleMap: newConcurrentHandleMap[BindingAnalyticsHost](),
}

func (c FfiConverterBindingAnalyticsHost) Lift(handle C.uint64_t) BindingAnalyticsHost {
	if uint64(handle)&1 == 0 {
		// Rust-generated handle (even), construct a new object wrapping the handle
		result := &BindingAnalyticsHostImpl{
			newFfiObject(
				handle,
				func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
					return C.uniffi_uc_engine_uniffi_fn_clone_bindinganalyticshost(handle, status)
				},
				func(handle C.uint64_t, status *C.RustCallStatus) {
					C.uniffi_uc_engine_uniffi_fn_free_bindinganalyticshost(handle, status)
				},
			),
		}
		runtime.SetFinalizer(result, (*BindingAnalyticsHostImpl).Destroy)
		return result
	} else {
		// Go-generated handle (odd), retrieve from the handle map
		val, ok := c.handleMap.tryGet(uint64(handle))
		if !ok {
			panic(fmt.Errorf("no callback in handle map: %d", handle))
		}
		c.handleMap.remove(uint64(handle))
		return val
	}
}

func (c FfiConverterBindingAnalyticsHost) Read(reader io.Reader) BindingAnalyticsHost {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterBindingAnalyticsHost) Lower(value BindingAnalyticsHost) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	if val, ok := value.(*BindingAnalyticsHostImpl); ok {
		// Rust-backed object, clone the handle
		handle := val.ffiObject.incrementPointer("BindingAnalyticsHost")
		defer val.ffiObject.decrementPointer()
		return handle
	} else {
		// Go-backed object, insert into handle map
		return C.uint64_t(c.handleMap.insert(value))
	}
}

func (c FfiConverterBindingAnalyticsHost) Write(writer io.Writer, value BindingAnalyticsHost) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalBindingAnalyticsHost(handle uint64) BindingAnalyticsHost {
	return FfiConverterBindingAnalyticsHostINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalBindingAnalyticsHost(value BindingAnalyticsHost) uint64 {
	return uint64(FfiConverterBindingAnalyticsHostINSTANCE.Lower(value))
}

type FfiDestroyerBindingAnalyticsHost struct{}

func (_ FfiDestroyerBindingAnalyticsHost) Destroy(value BindingAnalyticsHost) {
	if val, ok := value.(*BindingAnalyticsHostImpl); ok {
		val.Destroy()
	}
}

type uniffiCallbackResult C.int8_t

const (
	uniffiIdxCallbackFree               uniffiCallbackResult = 0
	uniffiCallbackResultSuccess         uniffiCallbackResult = 0
	uniffiCallbackResultError           uniffiCallbackResult = 1
	uniffiCallbackUnexpectedResultError uniffiCallbackResult = 2
	uniffiCallbackCancelled             uniffiCallbackResult = 3
)

type concurrentHandleMap[T any] struct {
	handles       map[uint64]T
	currentHandle uint64
	lock          sync.RWMutex
}

func newConcurrentHandleMap[T any]() *concurrentHandleMap[T] {
	return &concurrentHandleMap[T]{
		handles:       map[uint64]T{},
		currentHandle: 1,
	}
}

func (cm *concurrentHandleMap[T]) insert(obj T) uint64 {
	cm.lock.Lock()
	defer cm.lock.Unlock()

	handle := cm.currentHandle
	cm.currentHandle = cm.currentHandle + 2
	cm.handles[handle] = obj
	return handle
}

func (cm *concurrentHandleMap[T]) remove(handle uint64) {
	cm.lock.Lock()
	defer cm.lock.Unlock()

	delete(cm.handles, handle)
}

func (cm *concurrentHandleMap[T]) tryGet(handle uint64) (T, bool) {
	cm.lock.RLock()
	defer cm.lock.RUnlock()

	val, ok := cm.handles[handle]
	return val, ok
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod0
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod0(uniffiHandle C.uint64_t, event C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.Capture(
			FfiConverterBindingAnalyticsEventINSTANCE.Lift(GoRustBuffer{
				inner: event,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod1
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod1(uniffiHandle C.uint64_t, payload C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.Identify(
			FfiConverterBindingAnalyticsIdentifyINSTANCE.Lift(GoRustBuffer{
				inner: payload,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod2
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod2(uniffiHandle C.uint64_t, payload C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.GroupIdentify(
			FfiConverterBindingAnalyticsGroupIdentifyINSTANCE.Lift(GoRustBuffer{
				inner: payload,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod3
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod3(uniffiHandle C.uint64_t, spacePersonId C.RustBuffer, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.AdoptSpacePerson(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: spacePersonId,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod4
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod4(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.ReleaseSpacePerson()

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod5
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod5(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.CurrentSpacePersonId()

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterOptionalStringINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod6
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod6(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.ResetTelemetryIdentity()

	if _uniffiErr != nil {
		var _uniffiActualError *BindingAnalyticsHostError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterBindingAnalyticsHostErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBindingAnalyticsIdentityChangeINSTANCE.Lower(_uniffiRes)
}

var UniffiVTableCallbackInterfaceBindingAnalyticsHostINSTANCE = C.UniffiVTableCallbackInterfaceBindingAnalyticsHost{
	uniffiFree:             (C.UniffiCallbackInterfaceFree)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostFree),
	uniffiClone:            (C.UniffiCallbackInterfaceClone)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostClone),
	capture:                (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod0)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod0),
	identify:               (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod1)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod1),
	groupIdentify:          (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod2)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod2),
	adoptSpacePerson:       (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod3)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod3),
	releaseSpacePerson:     (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod4)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod4),
	currentSpacePersonId:   (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod5)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod5),
	resetTelemetryIdentity: (C.UniffiCallbackInterfaceBindingAnalyticsHostMethod6)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostMethod6),
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostFree
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostFree(handle C.uint64_t) {
	FfiConverterBindingAnalyticsHostINSTANCE.handleMap.remove(uint64(handle))
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostClone
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingAnalyticsHostClone(handle C.uint64_t) C.uint64_t {
	val, ok := FfiConverterBindingAnalyticsHostINSTANCE.handleMap.tryGet(uint64(handle))
	if !ok {
		panic(fmt.Errorf("no callback in handle map: %d", handle))
	}
	return C.uint64_t(FfiConverterBindingAnalyticsHostINSTANCE.handleMap.insert(val))
}

func (c FfiConverterBindingAnalyticsHost) register() {
	C.uniffi_uc_engine_uniffi_fn_init_callback_vtable_bindinganalyticshost(&UniffiVTableCallbackInterfaceBindingAnalyticsHostINSTANCE)
}

type BindingHost interface {
	PrivateDataDirectory() (string, error)
	CacheDirectory() (string, error)
	TemporaryDirectory() (string, error)
	SecureStorageGet(key string) (*[]byte, error)
	SecureStorageSet(key string, value []byte) error
	SecureStorageDelete(key string) error
	FileMetadata(handle string) (BindingFileMetadata, error)
	FileReadChunk(handle string, offset uint64, maxBytes uint32) ([]byte, error)
	FileWriteChunk(handle string, offset uint64, bytes []byte) error
	FileFinishWrite(handle string) error
	ClipboardRead() (BindingClipboardSnapshot, error)
	ClipboardWrite(snapshot BindingClipboardSnapshot) error
}
type BindingHostImpl struct {
	ffiObject FfiObject
}

func (_self *BindingHostImpl) PrivateDataDirectory() (string, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_private_data_directory(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) CacheDirectory() (string, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_cache_directory(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) TemporaryDirectory() (string, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_temporary_directory(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) SecureStorageGet(key string) (*[]byte, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_secure_storage_get(
				_pointer, FfiConverterStringINSTANCE.Lower(key), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *[]byte
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterOptionalBytesINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) SecureStorageSet(key string, value []byte) error {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinghost_secure_storage_set(
			_pointer, FfiConverterStringINSTANCE.Lower(key), FfiConverterBytesINSTANCE.Lower(value), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingHostImpl) SecureStorageDelete(key string) error {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinghost_secure_storage_delete(
			_pointer, FfiConverterStringINSTANCE.Lower(key), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingHostImpl) FileMetadata(handle string) (BindingFileMetadata, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_file_metadata(
				_pointer, FfiConverterStringINSTANCE.Lower(handle), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingFileMetadata
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingFileMetadataINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) FileReadChunk(handle string, offset uint64, maxBytes uint32) ([]byte, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_file_read_chunk(
				_pointer, FfiConverterStringINSTANCE.Lower(handle), FfiConverterUint64INSTANCE.Lower(offset), FfiConverterUint32INSTANCE.Lower(maxBytes), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue []byte
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBytesINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) FileWriteChunk(handle string, offset uint64, bytes []byte) error {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinghost_file_write_chunk(
			_pointer, FfiConverterStringINSTANCE.Lower(handle), FfiConverterUint64INSTANCE.Lower(offset), FfiConverterBytesINSTANCE.Lower(bytes), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingHostImpl) FileFinishWrite(handle string) error {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinghost_file_finish_write(
			_pointer, FfiConverterStringINSTANCE.Lower(handle), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *BindingHostImpl) ClipboardRead() (BindingClipboardSnapshot, error) {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_bindinghost_clipboard_read(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingClipboardSnapshot
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingClipboardSnapshotINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *BindingHostImpl) ClipboardWrite(snapshot BindingClipboardSnapshot) error {
	_pointer := _self.ffiObject.incrementPointer("BindingHost")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*HostBindingError](FfiConverterHostBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_bindinghost_clipboard_write(
			_pointer, FfiConverterBindingClipboardSnapshotINSTANCE.Lower(snapshot), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}
func (object *BindingHostImpl) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterBindingHost struct {
	handleMap *concurrentHandleMap[BindingHost]
}

var FfiConverterBindingHostINSTANCE = FfiConverterBindingHost{
	handleMap: newConcurrentHandleMap[BindingHost](),
}

func (c FfiConverterBindingHost) Lift(handle C.uint64_t) BindingHost {
	if uint64(handle)&1 == 0 {
		// Rust-generated handle (even), construct a new object wrapping the handle
		result := &BindingHostImpl{
			newFfiObject(
				handle,
				func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
					return C.uniffi_uc_engine_uniffi_fn_clone_bindinghost(handle, status)
				},
				func(handle C.uint64_t, status *C.RustCallStatus) {
					C.uniffi_uc_engine_uniffi_fn_free_bindinghost(handle, status)
				},
			),
		}
		runtime.SetFinalizer(result, (*BindingHostImpl).Destroy)
		return result
	} else {
		// Go-generated handle (odd), retrieve from the handle map
		val, ok := c.handleMap.tryGet(uint64(handle))
		if !ok {
			panic(fmt.Errorf("no callback in handle map: %d", handle))
		}
		c.handleMap.remove(uint64(handle))
		return val
	}
}

func (c FfiConverterBindingHost) Read(reader io.Reader) BindingHost {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterBindingHost) Lower(value BindingHost) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	if val, ok := value.(*BindingHostImpl); ok {
		// Rust-backed object, clone the handle
		handle := val.ffiObject.incrementPointer("BindingHost")
		defer val.ffiObject.decrementPointer()
		return handle
	} else {
		// Go-backed object, insert into handle map
		return C.uint64_t(c.handleMap.insert(value))
	}
}

func (c FfiConverterBindingHost) Write(writer io.Writer, value BindingHost) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalBindingHost(handle uint64) BindingHost {
	return FfiConverterBindingHostINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalBindingHost(value BindingHost) uint64 {
	return uint64(FfiConverterBindingHostINSTANCE.Lower(value))
}

type FfiDestroyerBindingHost struct{}

func (_ FfiDestroyerBindingHost) Destroy(value BindingHost) {
	if val, ok := value.(*BindingHostImpl); ok {
		val.Destroy()
	}
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod0
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod0(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.PrivateDataDirectory()

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterStringINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod1
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod1(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.CacheDirectory()

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterStringINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod2
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod2(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.TemporaryDirectory()

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterStringINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod3
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod3(uniffiHandle C.uint64_t, key C.RustBuffer, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.SecureStorageGet(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: key,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterOptionalBytesINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod4
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod4(uniffiHandle C.uint64_t, key C.RustBuffer, value C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.SecureStorageSet(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: key,
			}),
			FfiConverterBytesINSTANCE.Lift(GoRustBuffer{
				inner: value,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod5
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod5(uniffiHandle C.uint64_t, key C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.SecureStorageDelete(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: key,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod6
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod6(uniffiHandle C.uint64_t, handle C.RustBuffer, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.FileMetadata(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: handle,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBindingFileMetadataINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod7
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod7(uniffiHandle C.uint64_t, handle C.RustBuffer, offset C.uint64_t, maxBytes C.uint32_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.FileReadChunk(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: handle,
			}),
			FfiConverterUint64INSTANCE.Lift(offset),
			FfiConverterUint32INSTANCE.Lift(maxBytes),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBytesINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod8
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod8(uniffiHandle C.uint64_t, handle C.RustBuffer, offset C.uint64_t, bytes C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.FileWriteChunk(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: handle,
			}),
			FfiConverterUint64INSTANCE.Lift(offset),
			FfiConverterBytesINSTANCE.Lift(GoRustBuffer{
				inner: bytes,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod9
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod9(uniffiHandle C.uint64_t, handle C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.FileFinishWrite(
			FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: handle,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod10
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod10(uniffiHandle C.uint64_t, uniffiOutReturn *C.RustBuffer, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiRes, _uniffiErr :=
		_uniffiObj.ClipboardRead()

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

	*uniffiOutReturn = FfiConverterBindingClipboardSnapshotINSTANCE.Lower(_uniffiRes)
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod11
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod11(uniffiHandle C.uint64_t, snapshot C.RustBuffer, uniffiOutReturn *C.void, callStatus *C.RustCallStatus) {
	_uniffiCallbackHandle := uint64(uniffiHandle)
	_uniffiObj, _uniffiOk := FfiConverterBindingHostINSTANCE.handleMap.tryGet(_uniffiCallbackHandle)
	if !_uniffiOk {
		panic(fmt.Errorf("no callback in handle map: %d", _uniffiCallbackHandle))
	}

	_uniffiErr :=
		_uniffiObj.ClipboardWrite(
			FfiConverterBindingClipboardSnapshotINSTANCE.Lift(GoRustBuffer{
				inner: snapshot,
			}),
		)

	if _uniffiErr != nil {
		var _uniffiActualError *HostBindingError
		if errors.As(_uniffiErr, &_uniffiActualError) {
			*callStatus = C.RustCallStatus{
				code:     C.int8_t(uniffiCallbackResultError),
				errorBuf: FfiConverterHostBindingErrorINSTANCE.Lower(_uniffiActualError),
			}
		} else {
			*callStatus = C.RustCallStatus{
				code: C.int8_t(uniffiCallbackUnexpectedResultError),
			}
		}
		return
	}

}

var UniffiVTableCallbackInterfaceBindingHostINSTANCE = C.UniffiVTableCallbackInterfaceBindingHost{
	uniffiFree:           (C.UniffiCallbackInterfaceFree)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostFree),
	uniffiClone:          (C.UniffiCallbackInterfaceClone)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostClone),
	privateDataDirectory: (C.UniffiCallbackInterfaceBindingHostMethod0)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod0),
	cacheDirectory:       (C.UniffiCallbackInterfaceBindingHostMethod1)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod1),
	temporaryDirectory:   (C.UniffiCallbackInterfaceBindingHostMethod2)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod2),
	secureStorageGet:     (C.UniffiCallbackInterfaceBindingHostMethod3)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod3),
	secureStorageSet:     (C.UniffiCallbackInterfaceBindingHostMethod4)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod4),
	secureStorageDelete:  (C.UniffiCallbackInterfaceBindingHostMethod5)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod5),
	fileMetadata:         (C.UniffiCallbackInterfaceBindingHostMethod6)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod6),
	fileReadChunk:        (C.UniffiCallbackInterfaceBindingHostMethod7)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod7),
	fileWriteChunk:       (C.UniffiCallbackInterfaceBindingHostMethod8)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod8),
	fileFinishWrite:      (C.UniffiCallbackInterfaceBindingHostMethod9)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod9),
	clipboardRead:        (C.UniffiCallbackInterfaceBindingHostMethod10)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod10),
	clipboardWrite:       (C.UniffiCallbackInterfaceBindingHostMethod11)(C.uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostMethod11),
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostFree
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostFree(handle C.uint64_t) {
	FfiConverterBindingHostINSTANCE.handleMap.remove(uint64(handle))
}

//export uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostClone
func uc_engine_uniffi_cgo_dispatchCallbackInterfaceBindingHostClone(handle C.uint64_t) C.uint64_t {
	val, ok := FfiConverterBindingHostINSTANCE.handleMap.tryGet(uint64(handle))
	if !ok {
		panic(fmt.Errorf("no callback in handle map: %d", handle))
	}
	return C.uint64_t(FfiConverterBindingHostINSTANCE.handleMap.insert(val))
}

func (c FfiConverterBindingHost) register() {
	C.uniffi_uc_engine_uniffi_fn_init_callback_vtable_bindinghost(&UniffiVTableCallbackInterfaceBindingHostINSTANCE)
}

type MobileEngineInterface interface {
	AddCustomRelay(url string, accessToken string) (CustomRelayMutationResult, error)
	CancelJoinSpace(joinId string) (JoinSpaceStatus, error)
	CaptureCurrentClipboard() (*string, error)
	ChangeEncryptionPassphrase(passphrase string, passphraseConfirmation string) error
	ChooseDeviceGroup(issueId string, choiceId string, expectedRevision uint64, confirmLocalRemoval bool) (string, error)
	CreateSpace(deviceName *string, passphrase string) (SpaceCreated, error)
	DeleteCustomRelay(url string) (CustomRelayMutationResult, error)
	EditCustomRelay(previousUrl string, url string, accessToken string) (CustomRelayMutationResult, error)
	ExportEntry(entryId string, destinationHandle string) error
	IssueInvitation() (InvitationIssued, error)
	JoinSpace(invitationCode string, deviceName *string, passphrase string, preserveUnreadableHistory bool) (JoinSpaceStatus, error)
	LeaveSpace() error
	LifecycleState() (BindingEngineState, error)
	ListDevices() ([]Device, error)
	NextEvent(timeoutMs uint64) *BindingEvent
	NotifyConnectivityOpportunity(reason ConnectivityOpportunity) error
	ObserveClipboardChange(dispatch bool) (*SendReport, error)
	QueryActiveClipboard() (*ActiveClipboard, error)
	QueryCustomRelays() ([]CustomRelay, error)
	QueryDeviceGroupChoices() (string, error)
	QueryLocalDevice() (LocalDevice, error)
	QueryNetworkRecoveryStatus() (NetworkRecoveryStatus, error)
	QueryRelayOverview() (RelayOverview, error)
	QuerySpaceState() (SpaceState, error)
	RecoverNetwork() error
	RecoverSession(allowSecureStorageUnlock bool) (SessionRecovery, error)
	RefreshPeerConnections() (PeerConnectionRefresh, error)
	RemoveMember(deviceId string) (WorkspaceConvergence, error)
	ResendEntry(entryId string, targetDevices []string) (ResendEntryOutcome, error)
	RestoreClipboard(entryId string, mode BindingClipboardRestoreMode) (BindingClipboardRestoreOutcome, error)
	Resume() error
	SaveCustomRelay(url string, accessToken string, previousUrl *string) (RelaySaveResult, error)
	SendFiles(fileHandles []string, targetDevices []string) (SendReport, error)
	SendImage(bytes []byte, mimeType string, targetDevices []string) (SendReport, error)
	SendText(text string, targetDevices []string) (SendReport, error)
	Shutdown(deadlineMs uint64) error
	Suspend() error
	SuspendWithDeadline(deadlineMs uint64) error
}
type MobileEngine struct {
	ffiObject FfiObject
}

func MobileEngineStart(config BindingConfig, host BindingHost) (*MobileEngine, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_uc_engine_uniffi_fn_constructor_mobileengine_start(FfiConverterBindingConfigINSTANCE.Lower(config), FfiConverterBindingHostINSTANCE.Lower(host), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *MobileEngine
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterMobileEngineINSTANCE.Lift(_uniffiRV), nil
	}
}

func MobileEngineStartWithAnalytics(config BindingConfig, host BindingHost, analytics BindingAnalyticsHost, context BindingAnalyticsContext) (*MobileEngine, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_uc_engine_uniffi_fn_constructor_mobileengine_start_with_analytics(FfiConverterBindingConfigINSTANCE.Lower(config), FfiConverterBindingHostINSTANCE.Lower(host), FfiConverterBindingAnalyticsHostINSTANCE.Lower(analytics), FfiConverterBindingAnalyticsContextINSTANCE.Lower(context), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *MobileEngine
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterMobileEngineINSTANCE.Lift(_uniffiRV), nil
	}
}

func MobileEngineStartWithAnalyticsAndLifecycle(config BindingConfig, host BindingHost, analytics BindingAnalyticsHost, context BindingAnalyticsContext, lifecycle *MobileStartupLifecycle) (*MobileEngine, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_uc_engine_uniffi_fn_constructor_mobileengine_start_with_analytics_and_lifecycle(FfiConverterBindingConfigINSTANCE.Lower(config), FfiConverterBindingHostINSTANCE.Lower(host), FfiConverterBindingAnalyticsHostINSTANCE.Lower(analytics), FfiConverterBindingAnalyticsContextINSTANCE.Lower(context), FfiConverterMobileStartupLifecycleINSTANCE.Lower(lifecycle), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *MobileEngine
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterMobileEngineINSTANCE.Lift(_uniffiRV), nil
	}
}

func MobileEngineStartWithLifecycle(config BindingConfig, host BindingHost, lifecycle *MobileStartupLifecycle) (*MobileEngine, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_uc_engine_uniffi_fn_constructor_mobileengine_start_with_lifecycle(FfiConverterBindingConfigINSTANCE.Lower(config), FfiConverterBindingHostINSTANCE.Lower(host), FfiConverterMobileStartupLifecycleINSTANCE.Lower(lifecycle), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *MobileEngine
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterMobileEngineINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) AddCustomRelay(url string, accessToken string) (CustomRelayMutationResult, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_add_custom_relay(
				_pointer, FfiConverterStringINSTANCE.Lower(url), FfiConverterStringINSTANCE.Lower(accessToken), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue CustomRelayMutationResult
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterCustomRelayMutationResultINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) CancelJoinSpace(joinId string) (JoinSpaceStatus, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_cancel_join_space(
				_pointer, FfiConverterStringINSTANCE.Lower(joinId), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue JoinSpaceStatus
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterJoinSpaceStatusINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) CaptureCurrentClipboard() (*string, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_capture_current_clipboard(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterOptionalStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) ChangeEncryptionPassphrase(passphrase string, passphraseConfirmation string) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_change_encryption_passphrase(
			_pointer, FfiConverterStringINSTANCE.Lower(passphrase), FfiConverterStringINSTANCE.Lower(passphraseConfirmation), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) ChooseDeviceGroup(issueId string, choiceId string, expectedRevision uint64, confirmLocalRemoval bool) (string, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_choose_device_group(
				_pointer, FfiConverterStringINSTANCE.Lower(issueId), FfiConverterStringINSTANCE.Lower(choiceId), FfiConverterUint64INSTANCE.Lower(expectedRevision), FfiConverterBoolINSTANCE.Lower(confirmLocalRemoval), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) CreateSpace(deviceName *string, passphrase string) (SpaceCreated, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_create_space(
				_pointer, FfiConverterOptionalStringINSTANCE.Lower(deviceName), FfiConverterStringINSTANCE.Lower(passphrase), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SpaceCreated
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSpaceCreatedINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) DeleteCustomRelay(url string) (CustomRelayMutationResult, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_delete_custom_relay(
				_pointer, FfiConverterStringINSTANCE.Lower(url), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue CustomRelayMutationResult
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterCustomRelayMutationResultINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) EditCustomRelay(previousUrl string, url string, accessToken string) (CustomRelayMutationResult, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_edit_custom_relay(
				_pointer, FfiConverterStringINSTANCE.Lower(previousUrl), FfiConverterStringINSTANCE.Lower(url), FfiConverterStringINSTANCE.Lower(accessToken), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue CustomRelayMutationResult
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterCustomRelayMutationResultINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) ExportEntry(entryId string, destinationHandle string) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_export_entry(
			_pointer, FfiConverterStringINSTANCE.Lower(entryId), FfiConverterStringINSTANCE.Lower(destinationHandle), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) IssueInvitation() (InvitationIssued, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_issue_invitation(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue InvitationIssued
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterInvitationIssuedINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) JoinSpace(invitationCode string, deviceName *string, passphrase string, preserveUnreadableHistory bool) (JoinSpaceStatus, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_join_space(
				_pointer, FfiConverterStringINSTANCE.Lower(invitationCode), FfiConverterOptionalStringINSTANCE.Lower(deviceName), FfiConverterStringINSTANCE.Lower(passphrase), FfiConverterBoolINSTANCE.Lower(preserveUnreadableHistory), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue JoinSpaceStatus
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterJoinSpaceStatusINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) LeaveSpace() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_leave_space(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) LifecycleState() (BindingEngineState, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_lifecycle_state(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingEngineState
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingEngineStateINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) ListDevices() ([]Device, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_list_devices(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue []Device
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSequenceDeviceINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) NextEvent(timeoutMs uint64) *BindingEvent {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	return FfiConverterOptionalBindingEventINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_next_event(
				_pointer, FfiConverterUint64INSTANCE.Lower(timeoutMs), _uniffiStatus),
		}
	}))
}

func (_self *MobileEngine) NotifyConnectivityOpportunity(reason ConnectivityOpportunity) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_notify_connectivity_opportunity(
			_pointer, FfiConverterConnectivityOpportunityINSTANCE.Lower(reason), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) ObserveClipboardChange(dispatch bool) (*SendReport, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_observe_clipboard_change(
				_pointer, FfiConverterBoolINSTANCE.Lower(dispatch), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *SendReport
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterOptionalSendReportINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryActiveClipboard() (*ActiveClipboard, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_active_clipboard(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *ActiveClipboard
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterOptionalActiveClipboardINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryCustomRelays() ([]CustomRelay, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_custom_relays(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue []CustomRelay
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSequenceCustomRelayINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryDeviceGroupChoices() (string, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_device_group_choices(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue string
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterStringINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryLocalDevice() (LocalDevice, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_local_device(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue LocalDevice
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterLocalDeviceINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryNetworkRecoveryStatus() (NetworkRecoveryStatus, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_network_recovery_status(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue NetworkRecoveryStatus
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterNetworkRecoveryStatusINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QueryRelayOverview() (RelayOverview, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_relay_overview(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue RelayOverview
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterRelayOverviewINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) QuerySpaceState() (SpaceState, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_query_space_state(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SpaceState
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSpaceStateINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) RecoverNetwork() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_recover_network(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) RecoverSession(allowSecureStorageUnlock bool) (SessionRecovery, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_recover_session(
				_pointer, FfiConverterBoolINSTANCE.Lower(allowSecureStorageUnlock), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SessionRecovery
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSessionRecoveryINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) RefreshPeerConnections() (PeerConnectionRefresh, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_refresh_peer_connections(
				_pointer, _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue PeerConnectionRefresh
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterPeerConnectionRefreshINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) RemoveMember(deviceId string) (WorkspaceConvergence, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_remove_member(
				_pointer, FfiConverterStringINSTANCE.Lower(deviceId), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue WorkspaceConvergence
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterWorkspaceConvergenceINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) ResendEntry(entryId string, targetDevices []string) (ResendEntryOutcome, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_resend_entry(
				_pointer, FfiConverterStringINSTANCE.Lower(entryId), FfiConverterSequenceStringINSTANCE.Lower(targetDevices), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue ResendEntryOutcome
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterResendEntryOutcomeINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) RestoreClipboard(entryId string, mode BindingClipboardRestoreMode) (BindingClipboardRestoreOutcome, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_restore_clipboard(
				_pointer, FfiConverterStringINSTANCE.Lower(entryId), FfiConverterBindingClipboardRestoreModeINSTANCE.Lower(mode), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingClipboardRestoreOutcome
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingClipboardRestoreOutcomeINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) Resume() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_resume(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) SaveCustomRelay(url string, accessToken string, previousUrl *string) (RelaySaveResult, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_save_custom_relay(
				_pointer, FfiConverterStringINSTANCE.Lower(url), FfiConverterStringINSTANCE.Lower(accessToken), FfiConverterOptionalStringINSTANCE.Lower(previousUrl), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue RelaySaveResult
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterRelaySaveResultINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) SendFiles(fileHandles []string, targetDevices []string) (SendReport, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_send_files(
				_pointer, FfiConverterSequenceStringINSTANCE.Lower(fileHandles), FfiConverterSequenceStringINSTANCE.Lower(targetDevices), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SendReport
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSendReportINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) SendImage(bytes []byte, mimeType string, targetDevices []string) (SendReport, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_send_image(
				_pointer, FfiConverterBytesINSTANCE.Lower(bytes), FfiConverterStringINSTANCE.Lower(mimeType), FfiConverterSequenceStringINSTANCE.Lower(targetDevices), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SendReport
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSendReportINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) SendText(text string, targetDevices []string) (SendReport, error) {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_method_mobileengine_send_text(
				_pointer, FfiConverterStringINSTANCE.Lower(text), FfiConverterSequenceStringINSTANCE.Lower(targetDevices), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue SendReport
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterSendReportINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *MobileEngine) Shutdown(deadlineMs uint64) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_shutdown(
			_pointer, FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) Suspend() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_suspend(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileEngine) SuspendWithDeadline(deadlineMs uint64) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileEngine")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobileengine_suspend_with_deadline(
			_pointer, FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}
func (object *MobileEngine) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterMobileEngine struct{}

var FfiConverterMobileEngineINSTANCE = FfiConverterMobileEngine{}

func (c FfiConverterMobileEngine) Lift(handle C.uint64_t) *MobileEngine {
	result := &MobileEngine{
		newFfiObject(
			handle,
			func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
				return C.uniffi_uc_engine_uniffi_fn_clone_mobileengine(handle, status)
			},
			func(handle C.uint64_t, status *C.RustCallStatus) {
				C.uniffi_uc_engine_uniffi_fn_free_mobileengine(handle, status)
			},
		),
	}
	runtime.SetFinalizer(result, (*MobileEngine).Destroy)
	return result
}

func (c FfiConverterMobileEngine) Read(reader io.Reader) *MobileEngine {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterMobileEngine) Lower(value *MobileEngine) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	handle := value.ffiObject.incrementPointer("*MobileEngine")
	defer value.ffiObject.decrementPointer()
	return handle
}

func (c FfiConverterMobileEngine) Write(writer io.Writer, value *MobileEngine) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalMobileEngine(handle uint64) *MobileEngine {
	return FfiConverterMobileEngineINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalMobileEngine(value *MobileEngine) uint64 {
	return uint64(FfiConverterMobileEngineINSTANCE.Lower(value))
}

type FfiDestroyerMobileEngine struct{}

func (_ FfiDestroyerMobileEngine) Destroy(value *MobileEngine) {
	value.Destroy()
}

// 宿主在启动调用尚未返回时转交暂停和恢复通知。
type MobileStartupLifecycleInterface interface {
	Resume() error
	Suspend() error
	SuspendWithDeadline(deadlineMs uint64) error
}

// 宿主在启动调用尚未返回时转交暂停和恢复通知。
type MobileStartupLifecycle struct {
	ffiObject FfiObject
}

func NewMobileStartupLifecycle() *MobileStartupLifecycle {
	return FfiConverterMobileStartupLifecycleINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_uc_engine_uniffi_fn_constructor_mobilestartuplifecycle_new(_uniffiStatus)
	}))
}

func (_self *MobileStartupLifecycle) Resume() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileStartupLifecycle")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobilestartuplifecycle_resume(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileStartupLifecycle) Suspend() error {
	_pointer := _self.ffiObject.incrementPointer("*MobileStartupLifecycle")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobilestartuplifecycle_suspend(
			_pointer, _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func (_self *MobileStartupLifecycle) SuspendWithDeadline(deadlineMs uint64) error {
	_pointer := _self.ffiObject.incrementPointer("*MobileStartupLifecycle")
	defer _self.ffiObject.decrementPointer()
	_, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_method_mobilestartuplifecycle_suspend_with_deadline(
			_pointer, FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}
func (object *MobileStartupLifecycle) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterMobileStartupLifecycle struct{}

var FfiConverterMobileStartupLifecycleINSTANCE = FfiConverterMobileStartupLifecycle{}

func (c FfiConverterMobileStartupLifecycle) Lift(handle C.uint64_t) *MobileStartupLifecycle {
	result := &MobileStartupLifecycle{
		newFfiObject(
			handle,
			func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
				return C.uniffi_uc_engine_uniffi_fn_clone_mobilestartuplifecycle(handle, status)
			},
			func(handle C.uint64_t, status *C.RustCallStatus) {
				C.uniffi_uc_engine_uniffi_fn_free_mobilestartuplifecycle(handle, status)
			},
		),
	}
	runtime.SetFinalizer(result, (*MobileStartupLifecycle).Destroy)
	return result
}

func (c FfiConverterMobileStartupLifecycle) Read(reader io.Reader) *MobileStartupLifecycle {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterMobileStartupLifecycle) Lower(value *MobileStartupLifecycle) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	handle := value.ffiObject.incrementPointer("*MobileStartupLifecycle")
	defer value.ffiObject.decrementPointer()
	return handle
}

func (c FfiConverterMobileStartupLifecycle) Write(writer io.Writer, value *MobileStartupLifecycle) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalMobileStartupLifecycle(handle uint64) *MobileStartupLifecycle {
	return FfiConverterMobileStartupLifecycleINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalMobileStartupLifecycle(value *MobileStartupLifecycle) uint64 {
	return uint64(FfiConverterMobileStartupLifecycleINSTANCE.Lower(value))
}

type FfiDestroyerMobileStartupLifecycle struct{}

func (_ FfiDestroyerMobileStartupLifecycle) Destroy(value *MobileStartupLifecycle) {
	value.Destroy()
}

type ActiveClipboard struct {
	EntryId     string
	ActivatedBy string
}

func (r *ActiveClipboard) Destroy() {
	FfiDestroyerString{}.Destroy(r.EntryId)
	FfiDestroyerString{}.Destroy(r.ActivatedBy)
}

type FfiConverterActiveClipboard struct{}

var FfiConverterActiveClipboardINSTANCE = FfiConverterActiveClipboard{}

func (c FfiConverterActiveClipboard) Lift(rb RustBufferI) ActiveClipboard {
	return LiftFromRustBuffer[ActiveClipboard](c, rb)
}

func (c FfiConverterActiveClipboard) Read(reader io.Reader) ActiveClipboard {
	return ActiveClipboard{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterActiveClipboard) Lower(value ActiveClipboard) C.RustBuffer {
	return LowerIntoRustBuffer[ActiveClipboard](c, value)
}

func (c FfiConverterActiveClipboard) LowerExternal(value ActiveClipboard) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ActiveClipboard](c, value))
}

func (c FfiConverterActiveClipboard) Write(writer io.Writer, value ActiveClipboard) {
	FfiConverterStringINSTANCE.Write(writer, value.EntryId)
	FfiConverterStringINSTANCE.Write(writer, value.ActivatedBy)
}

type FfiDestroyerActiveClipboard struct{}

func (_ FfiDestroyerActiveClipboard) Destroy(value ActiveClipboard) {
	value.Destroy()
}

// Platform attributes that the host supplies once for all analytics events.
type BindingAnalyticsContext struct {
	Os         BindingAnalyticsOs
	OsVersion  string
	DeviceType BindingAnalyticsDeviceType
	Arch       string
	AppChannel string
}

func (r *BindingAnalyticsContext) Destroy() {
	FfiDestroyerBindingAnalyticsOs{}.Destroy(r.Os)
	FfiDestroyerString{}.Destroy(r.OsVersion)
	FfiDestroyerBindingAnalyticsDeviceType{}.Destroy(r.DeviceType)
	FfiDestroyerString{}.Destroy(r.Arch)
	FfiDestroyerString{}.Destroy(r.AppChannel)
}

type FfiConverterBindingAnalyticsContext struct{}

var FfiConverterBindingAnalyticsContextINSTANCE = FfiConverterBindingAnalyticsContext{}

func (c FfiConverterBindingAnalyticsContext) Lift(rb RustBufferI) BindingAnalyticsContext {
	return LiftFromRustBuffer[BindingAnalyticsContext](c, rb)
}

func (c FfiConverterBindingAnalyticsContext) Read(reader io.Reader) BindingAnalyticsContext {
	return BindingAnalyticsContext{
		FfiConverterBindingAnalyticsOsINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBindingAnalyticsDeviceTypeINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingAnalyticsContext) Lower(value BindingAnalyticsContext) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsContext](c, value)
}

func (c FfiConverterBindingAnalyticsContext) LowerExternal(value BindingAnalyticsContext) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsContext](c, value))
}

func (c FfiConverterBindingAnalyticsContext) Write(writer io.Writer, value BindingAnalyticsContext) {
	FfiConverterBindingAnalyticsOsINSTANCE.Write(writer, value.Os)
	FfiConverterStringINSTANCE.Write(writer, value.OsVersion)
	FfiConverterBindingAnalyticsDeviceTypeINSTANCE.Write(writer, value.DeviceType)
	FfiConverterStringINSTANCE.Write(writer, value.Arch)
	FfiConverterStringINSTANCE.Write(writer, value.AppChannel)
}

type FfiDestroyerBindingAnalyticsContext struct{}

func (_ FfiDestroyerBindingAnalyticsContext) Destroy(value BindingAnalyticsContext) {
	value.Destroy()
}

type BindingAnalyticsEvent struct {
	Name           string
	PropertiesJson string
}

func (r *BindingAnalyticsEvent) Destroy() {
	FfiDestroyerString{}.Destroy(r.Name)
	FfiDestroyerString{}.Destroy(r.PropertiesJson)
}

type FfiConverterBindingAnalyticsEvent struct{}

var FfiConverterBindingAnalyticsEventINSTANCE = FfiConverterBindingAnalyticsEvent{}

func (c FfiConverterBindingAnalyticsEvent) Lift(rb RustBufferI) BindingAnalyticsEvent {
	return LiftFromRustBuffer[BindingAnalyticsEvent](c, rb)
}

func (c FfiConverterBindingAnalyticsEvent) Read(reader io.Reader) BindingAnalyticsEvent {
	return BindingAnalyticsEvent{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingAnalyticsEvent) Lower(value BindingAnalyticsEvent) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsEvent](c, value)
}

func (c FfiConverterBindingAnalyticsEvent) LowerExternal(value BindingAnalyticsEvent) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsEvent](c, value))
}

func (c FfiConverterBindingAnalyticsEvent) Write(writer io.Writer, value BindingAnalyticsEvent) {
	FfiConverterStringINSTANCE.Write(writer, value.Name)
	FfiConverterStringINSTANCE.Write(writer, value.PropertiesJson)
}

type FfiDestroyerBindingAnalyticsEvent struct{}

func (_ FfiDestroyerBindingAnalyticsEvent) Destroy(value BindingAnalyticsEvent) {
	value.Destroy()
}

type BindingAnalyticsGroupIdentify struct {
	GroupType string
	GroupKey  string
	SetJson   string
}

func (r *BindingAnalyticsGroupIdentify) Destroy() {
	FfiDestroyerString{}.Destroy(r.GroupType)
	FfiDestroyerString{}.Destroy(r.GroupKey)
	FfiDestroyerString{}.Destroy(r.SetJson)
}

type FfiConverterBindingAnalyticsGroupIdentify struct{}

var FfiConverterBindingAnalyticsGroupIdentifyINSTANCE = FfiConverterBindingAnalyticsGroupIdentify{}

func (c FfiConverterBindingAnalyticsGroupIdentify) Lift(rb RustBufferI) BindingAnalyticsGroupIdentify {
	return LiftFromRustBuffer[BindingAnalyticsGroupIdentify](c, rb)
}

func (c FfiConverterBindingAnalyticsGroupIdentify) Read(reader io.Reader) BindingAnalyticsGroupIdentify {
	return BindingAnalyticsGroupIdentify{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingAnalyticsGroupIdentify) Lower(value BindingAnalyticsGroupIdentify) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsGroupIdentify](c, value)
}

func (c FfiConverterBindingAnalyticsGroupIdentify) LowerExternal(value BindingAnalyticsGroupIdentify) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsGroupIdentify](c, value))
}

func (c FfiConverterBindingAnalyticsGroupIdentify) Write(writer io.Writer, value BindingAnalyticsGroupIdentify) {
	FfiConverterStringINSTANCE.Write(writer, value.GroupType)
	FfiConverterStringINSTANCE.Write(writer, value.GroupKey)
	FfiConverterStringINSTANCE.Write(writer, value.SetJson)
}

type FfiDestroyerBindingAnalyticsGroupIdentify struct{}

func (_ FfiDestroyerBindingAnalyticsGroupIdentify) Destroy(value BindingAnalyticsGroupIdentify) {
	value.Destroy()
}

type BindingAnalyticsIdentify struct {
	OldDistinctId string
	NewDistinctId string
	SetJson       string
	SetOnceJson   string
}

func (r *BindingAnalyticsIdentify) Destroy() {
	FfiDestroyerString{}.Destroy(r.OldDistinctId)
	FfiDestroyerString{}.Destroy(r.NewDistinctId)
	FfiDestroyerString{}.Destroy(r.SetJson)
	FfiDestroyerString{}.Destroy(r.SetOnceJson)
}

type FfiConverterBindingAnalyticsIdentify struct{}

var FfiConverterBindingAnalyticsIdentifyINSTANCE = FfiConverterBindingAnalyticsIdentify{}

func (c FfiConverterBindingAnalyticsIdentify) Lift(rb RustBufferI) BindingAnalyticsIdentify {
	return LiftFromRustBuffer[BindingAnalyticsIdentify](c, rb)
}

func (c FfiConverterBindingAnalyticsIdentify) Read(reader io.Reader) BindingAnalyticsIdentify {
	return BindingAnalyticsIdentify{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingAnalyticsIdentify) Lower(value BindingAnalyticsIdentify) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsIdentify](c, value)
}

func (c FfiConverterBindingAnalyticsIdentify) LowerExternal(value BindingAnalyticsIdentify) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsIdentify](c, value))
}

func (c FfiConverterBindingAnalyticsIdentify) Write(writer io.Writer, value BindingAnalyticsIdentify) {
	FfiConverterStringINSTANCE.Write(writer, value.OldDistinctId)
	FfiConverterStringINSTANCE.Write(writer, value.NewDistinctId)
	FfiConverterStringINSTANCE.Write(writer, value.SetJson)
	FfiConverterStringINSTANCE.Write(writer, value.SetOnceJson)
}

type FfiDestroyerBindingAnalyticsIdentify struct{}

func (_ FfiDestroyerBindingAnalyticsIdentify) Destroy(value BindingAnalyticsIdentify) {
	value.Destroy()
}

type BindingAnalyticsIdentityChange struct {
	PreviousDistinctId string
	NewDistinctId      string
}

func (r *BindingAnalyticsIdentityChange) Destroy() {
	FfiDestroyerString{}.Destroy(r.PreviousDistinctId)
	FfiDestroyerString{}.Destroy(r.NewDistinctId)
}

type FfiConverterBindingAnalyticsIdentityChange struct{}

var FfiConverterBindingAnalyticsIdentityChangeINSTANCE = FfiConverterBindingAnalyticsIdentityChange{}

func (c FfiConverterBindingAnalyticsIdentityChange) Lift(rb RustBufferI) BindingAnalyticsIdentityChange {
	return LiftFromRustBuffer[BindingAnalyticsIdentityChange](c, rb)
}

func (c FfiConverterBindingAnalyticsIdentityChange) Read(reader io.Reader) BindingAnalyticsIdentityChange {
	return BindingAnalyticsIdentityChange{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingAnalyticsIdentityChange) Lower(value BindingAnalyticsIdentityChange) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsIdentityChange](c, value)
}

func (c FfiConverterBindingAnalyticsIdentityChange) LowerExternal(value BindingAnalyticsIdentityChange) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsIdentityChange](c, value))
}

func (c FfiConverterBindingAnalyticsIdentityChange) Write(writer io.Writer, value BindingAnalyticsIdentityChange) {
	FfiConverterStringINSTANCE.Write(writer, value.PreviousDistinctId)
	FfiConverterStringINSTANCE.Write(writer, value.NewDistinctId)
}

type FfiDestroyerBindingAnalyticsIdentityChange struct{}

func (_ FfiDestroyerBindingAnalyticsIdentityChange) Destroy(value BindingAnalyticsIdentityChange) {
	value.Destroy()
}

type BindingClipboardSnapshot struct {
	ObservedAtMs    int64
	Representations []BindingClipboardRepresentation
}

func (r *BindingClipboardSnapshot) Destroy() {
	FfiDestroyerInt64{}.Destroy(r.ObservedAtMs)
	FfiDestroyerSequenceBindingClipboardRepresentation{}.Destroy(r.Representations)
}

type FfiConverterBindingClipboardSnapshot struct{}

var FfiConverterBindingClipboardSnapshotINSTANCE = FfiConverterBindingClipboardSnapshot{}

func (c FfiConverterBindingClipboardSnapshot) Lift(rb RustBufferI) BindingClipboardSnapshot {
	return LiftFromRustBuffer[BindingClipboardSnapshot](c, rb)
}

func (c FfiConverterBindingClipboardSnapshot) Read(reader io.Reader) BindingClipboardSnapshot {
	return BindingClipboardSnapshot{
		FfiConverterInt64INSTANCE.Read(reader),
		FfiConverterSequenceBindingClipboardRepresentationINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingClipboardSnapshot) Lower(value BindingClipboardSnapshot) C.RustBuffer {
	return LowerIntoRustBuffer[BindingClipboardSnapshot](c, value)
}

func (c FfiConverterBindingClipboardSnapshot) LowerExternal(value BindingClipboardSnapshot) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingClipboardSnapshot](c, value))
}

func (c FfiConverterBindingClipboardSnapshot) Write(writer io.Writer, value BindingClipboardSnapshot) {
	FfiConverterInt64INSTANCE.Write(writer, value.ObservedAtMs)
	FfiConverterSequenceBindingClipboardRepresentationINSTANCE.Write(writer, value.Representations)
}

type FfiDestroyerBindingClipboardSnapshot struct{}

func (_ FfiDestroyerBindingClipboardSnapshot) Destroy(value BindingClipboardSnapshot) {
	value.Destroy()
}

type BindingCollectorConfig struct {
	TraceEndpoint   string
	LogEndpoint     string
	AuthHeaderName  *string
	AuthHeaderValue *string
}

func (r *BindingCollectorConfig) Destroy() {
	FfiDestroyerString{}.Destroy(r.TraceEndpoint)
	FfiDestroyerString{}.Destroy(r.LogEndpoint)
	FfiDestroyerOptionalString{}.Destroy(r.AuthHeaderName)
	FfiDestroyerOptionalString{}.Destroy(r.AuthHeaderValue)
}

type FfiConverterBindingCollectorConfig struct{}

var FfiConverterBindingCollectorConfigINSTANCE = FfiConverterBindingCollectorConfig{}

func (c FfiConverterBindingCollectorConfig) Lift(rb RustBufferI) BindingCollectorConfig {
	return LiftFromRustBuffer[BindingCollectorConfig](c, rb)
}

func (c FfiConverterBindingCollectorConfig) Read(reader io.Reader) BindingCollectorConfig {
	return BindingCollectorConfig{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingCollectorConfig) Lower(value BindingCollectorConfig) C.RustBuffer {
	return LowerIntoRustBuffer[BindingCollectorConfig](c, value)
}

func (c FfiConverterBindingCollectorConfig) LowerExternal(value BindingCollectorConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingCollectorConfig](c, value))
}

func (c FfiConverterBindingCollectorConfig) Write(writer io.Writer, value BindingCollectorConfig) {
	FfiConverterStringINSTANCE.Write(writer, value.TraceEndpoint)
	FfiConverterStringINSTANCE.Write(writer, value.LogEndpoint)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.AuthHeaderName)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.AuthHeaderValue)
}

type FfiDestroyerBindingCollectorConfig struct{}

func (_ FfiDestroyerBindingCollectorConfig) Destroy(value BindingCollectorConfig) {
	value.Destroy()
}

type BindingConfig struct {
	AppVersion string
	ProfileId  string
}

func (r *BindingConfig) Destroy() {
	FfiDestroyerString{}.Destroy(r.AppVersion)
	FfiDestroyerString{}.Destroy(r.ProfileId)
}

type FfiConverterBindingConfig struct{}

var FfiConverterBindingConfigINSTANCE = FfiConverterBindingConfig{}

func (c FfiConverterBindingConfig) Lift(rb RustBufferI) BindingConfig {
	return LiftFromRustBuffer[BindingConfig](c, rb)
}

func (c FfiConverterBindingConfig) Read(reader io.Reader) BindingConfig {
	return BindingConfig{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingConfig) Lower(value BindingConfig) C.RustBuffer {
	return LowerIntoRustBuffer[BindingConfig](c, value)
}

func (c FfiConverterBindingConfig) LowerExternal(value BindingConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingConfig](c, value))
}

func (c FfiConverterBindingConfig) Write(writer io.Writer, value BindingConfig) {
	FfiConverterStringINSTANCE.Write(writer, value.AppVersion)
	FfiConverterStringINSTANCE.Write(writer, value.ProfileId)
}

type FfiDestroyerBindingConfig struct{}

func (_ FfiDestroyerBindingConfig) Destroy(value BindingConfig) {
	value.Destroy()
}

type BindingFailure struct {
	Code      uint32
	Category  BindingErrorCategory
	Retryable bool
}

func (r *BindingFailure) Destroy() {
	FfiDestroyerUint32{}.Destroy(r.Code)
	FfiDestroyerBindingErrorCategory{}.Destroy(r.Category)
	FfiDestroyerBool{}.Destroy(r.Retryable)
}

type FfiConverterBindingFailure struct{}

var FfiConverterBindingFailureINSTANCE = FfiConverterBindingFailure{}

func (c FfiConverterBindingFailure) Lift(rb RustBufferI) BindingFailure {
	return LiftFromRustBuffer[BindingFailure](c, rb)
}

func (c FfiConverterBindingFailure) Read(reader io.Reader) BindingFailure {
	return BindingFailure{
		FfiConverterUint32INSTANCE.Read(reader),
		FfiConverterBindingErrorCategoryINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingFailure) Lower(value BindingFailure) C.RustBuffer {
	return LowerIntoRustBuffer[BindingFailure](c, value)
}

func (c FfiConverterBindingFailure) LowerExternal(value BindingFailure) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingFailure](c, value))
}

func (c FfiConverterBindingFailure) Write(writer io.Writer, value BindingFailure) {
	FfiConverterUint32INSTANCE.Write(writer, value.Code)
	FfiConverterBindingErrorCategoryINSTANCE.Write(writer, value.Category)
	FfiConverterBoolINSTANCE.Write(writer, value.Retryable)
}

type FfiDestroyerBindingFailure struct{}

func (_ FfiDestroyerBindingFailure) Destroy(value BindingFailure) {
	value.Destroy()
}

type BindingFileMetadata struct {
	DisplayName string
	SizeBytes   uint64
	MimeType    *string
}

func (r *BindingFileMetadata) Destroy() {
	FfiDestroyerString{}.Destroy(r.DisplayName)
	FfiDestroyerUint64{}.Destroy(r.SizeBytes)
	FfiDestroyerOptionalString{}.Destroy(r.MimeType)
}

type FfiConverterBindingFileMetadata struct{}

var FfiConverterBindingFileMetadataINSTANCE = FfiConverterBindingFileMetadata{}

func (c FfiConverterBindingFileMetadata) Lift(rb RustBufferI) BindingFileMetadata {
	return LiftFromRustBuffer[BindingFileMetadata](c, rb)
}

func (c FfiConverterBindingFileMetadata) Read(reader io.Reader) BindingFileMetadata {
	return BindingFileMetadata{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingFileMetadata) Lower(value BindingFileMetadata) C.RustBuffer {
	return LowerIntoRustBuffer[BindingFileMetadata](c, value)
}

func (c FfiConverterBindingFileMetadata) LowerExternal(value BindingFileMetadata) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingFileMetadata](c, value))
}

func (c FfiConverterBindingFileMetadata) Write(writer io.Writer, value BindingFileMetadata) {
	FfiConverterStringINSTANCE.Write(writer, value.DisplayName)
	FfiConverterUint64INSTANCE.Write(writer, value.SizeBytes)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.MimeType)
}

type FfiDestroyerBindingFileMetadata struct{}

func (_ FfiDestroyerBindingFileMetadata) Destroy(value BindingFileMetadata) {
	value.Destroy()
}

type BindingFileSourceCounts struct {
	Source            BindingLocalDiagnosticSource
	AcceptedCount     uint64
	WrittenCount      uint64
	QueueDroppedCount uint64
	QuotaDroppedCount uint64
	WriteFailedCount  uint64
	LastWrittenAtMs   *uint64
}

func (r *BindingFileSourceCounts) Destroy() {
	FfiDestroyerBindingLocalDiagnosticSource{}.Destroy(r.Source)
	FfiDestroyerUint64{}.Destroy(r.AcceptedCount)
	FfiDestroyerUint64{}.Destroy(r.WrittenCount)
	FfiDestroyerUint64{}.Destroy(r.QueueDroppedCount)
	FfiDestroyerUint64{}.Destroy(r.QuotaDroppedCount)
	FfiDestroyerUint64{}.Destroy(r.WriteFailedCount)
	FfiDestroyerOptionalUint64{}.Destroy(r.LastWrittenAtMs)
}

type FfiConverterBindingFileSourceCounts struct{}

var FfiConverterBindingFileSourceCountsINSTANCE = FfiConverterBindingFileSourceCounts{}

func (c FfiConverterBindingFileSourceCounts) Lift(rb RustBufferI) BindingFileSourceCounts {
	return LiftFromRustBuffer[BindingFileSourceCounts](c, rb)
}

func (c FfiConverterBindingFileSourceCounts) Read(reader io.Reader) BindingFileSourceCounts {
	return BindingFileSourceCounts{
		FfiConverterBindingLocalDiagnosticSourceINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterOptionalUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingFileSourceCounts) Lower(value BindingFileSourceCounts) C.RustBuffer {
	return LowerIntoRustBuffer[BindingFileSourceCounts](c, value)
}

func (c FfiConverterBindingFileSourceCounts) LowerExternal(value BindingFileSourceCounts) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingFileSourceCounts](c, value))
}

func (c FfiConverterBindingFileSourceCounts) Write(writer io.Writer, value BindingFileSourceCounts) {
	FfiConverterBindingLocalDiagnosticSourceINSTANCE.Write(writer, value.Source)
	FfiConverterUint64INSTANCE.Write(writer, value.AcceptedCount)
	FfiConverterUint64INSTANCE.Write(writer, value.WrittenCount)
	FfiConverterUint64INSTANCE.Write(writer, value.QueueDroppedCount)
	FfiConverterUint64INSTANCE.Write(writer, value.QuotaDroppedCount)
	FfiConverterUint64INSTANCE.Write(writer, value.WriteFailedCount)
	FfiConverterOptionalUint64INSTANCE.Write(writer, value.LastWrittenAtMs)
}

type FfiDestroyerBindingFileSourceCounts struct{}

func (_ FfiDestroyerBindingFileSourceCounts) Destroy(value BindingFileSourceCounts) {
	value.Destroy()
}

type BindingHostDiagnosticReceipt struct {
	Status BindingHostDiagnosticRecordStatus
	Token  *string
}

func (r *BindingHostDiagnosticReceipt) Destroy() {
	FfiDestroyerBindingHostDiagnosticRecordStatus{}.Destroy(r.Status)
	FfiDestroyerOptionalString{}.Destroy(r.Token)
}

type FfiConverterBindingHostDiagnosticReceipt struct{}

var FfiConverterBindingHostDiagnosticReceiptINSTANCE = FfiConverterBindingHostDiagnosticReceipt{}

func (c FfiConverterBindingHostDiagnosticReceipt) Lift(rb RustBufferI) BindingHostDiagnosticReceipt {
	return LiftFromRustBuffer[BindingHostDiagnosticReceipt](c, rb)
}

func (c FfiConverterBindingHostDiagnosticReceipt) Read(reader io.Reader) BindingHostDiagnosticReceipt {
	return BindingHostDiagnosticReceipt{
		FfiConverterBindingHostDiagnosticRecordStatusINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingHostDiagnosticReceipt) Lower(value BindingHostDiagnosticReceipt) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticReceipt](c, value)
}

func (c FfiConverterBindingHostDiagnosticReceipt) LowerExternal(value BindingHostDiagnosticReceipt) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticReceipt](c, value))
}

func (c FfiConverterBindingHostDiagnosticReceipt) Write(writer io.Writer, value BindingHostDiagnosticReceipt) {
	FfiConverterBindingHostDiagnosticRecordStatusINSTANCE.Write(writer, value.Status)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Token)
}

type FfiDestroyerBindingHostDiagnosticReceipt struct{}

func (_ FfiDestroyerBindingHostDiagnosticReceipt) Destroy(value BindingHostDiagnosticReceipt) {
	value.Destroy()
}

type BindingLocalCaptureStatus struct {
	Mode          BindingLocalCaptureMode
	CaptureId     *string
	RemainingMs   uint64
	StartedAtUtc  *string
	EndReason     *BindingCaptureEndReason
	LastCaptureId *string
	Revision      uint64
}

func (r *BindingLocalCaptureStatus) Destroy() {
	FfiDestroyerBindingLocalCaptureMode{}.Destroy(r.Mode)
	FfiDestroyerOptionalString{}.Destroy(r.CaptureId)
	FfiDestroyerUint64{}.Destroy(r.RemainingMs)
	FfiDestroyerOptionalString{}.Destroy(r.StartedAtUtc)
	FfiDestroyerOptionalBindingCaptureEndReason{}.Destroy(r.EndReason)
	FfiDestroyerOptionalString{}.Destroy(r.LastCaptureId)
	FfiDestroyerUint64{}.Destroy(r.Revision)
}

type FfiConverterBindingLocalCaptureStatus struct{}

var FfiConverterBindingLocalCaptureStatusINSTANCE = FfiConverterBindingLocalCaptureStatus{}

func (c FfiConverterBindingLocalCaptureStatus) Lift(rb RustBufferI) BindingLocalCaptureStatus {
	return LiftFromRustBuffer[BindingLocalCaptureStatus](c, rb)
}

func (c FfiConverterBindingLocalCaptureStatus) Read(reader io.Reader) BindingLocalCaptureStatus {
	return BindingLocalCaptureStatus{
		FfiConverterBindingLocalCaptureModeINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalBindingCaptureEndReasonINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingLocalCaptureStatus) Lower(value BindingLocalCaptureStatus) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLocalCaptureStatus](c, value)
}

func (c FfiConverterBindingLocalCaptureStatus) LowerExternal(value BindingLocalCaptureStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLocalCaptureStatus](c, value))
}

func (c FfiConverterBindingLocalCaptureStatus) Write(writer io.Writer, value BindingLocalCaptureStatus) {
	FfiConverterBindingLocalCaptureModeINSTANCE.Write(writer, value.Mode)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.CaptureId)
	FfiConverterUint64INSTANCE.Write(writer, value.RemainingMs)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.StartedAtUtc)
	FfiConverterOptionalBindingCaptureEndReasonINSTANCE.Write(writer, value.EndReason)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.LastCaptureId)
	FfiConverterUint64INSTANCE.Write(writer, value.Revision)
}

type FfiDestroyerBindingLocalCaptureStatus struct{}

func (_ FfiDestroyerBindingLocalCaptureStatus) Destroy(value BindingLocalCaptureStatus) {
	value.Destroy()
}

type BindingLocalDiagnosticExportReport struct {
	Flush                 BindingObservabilitySignalResult
	Status                BindingLocalDiagnosticStatus
	RequestedAtUtc        string
	CompletedAtUtc        string
	OtherProcessesFlushed bool
	Files                 []BindingFileSourceCounts
}

func (r *BindingLocalDiagnosticExportReport) Destroy() {
	FfiDestroyerBindingObservabilitySignalResult{}.Destroy(r.Flush)
	FfiDestroyerBindingLocalDiagnosticStatus{}.Destroy(r.Status)
	FfiDestroyerString{}.Destroy(r.RequestedAtUtc)
	FfiDestroyerString{}.Destroy(r.CompletedAtUtc)
	FfiDestroyerBool{}.Destroy(r.OtherProcessesFlushed)
	FfiDestroyerSequenceBindingFileSourceCounts{}.Destroy(r.Files)
}

type FfiConverterBindingLocalDiagnosticExportReport struct{}

var FfiConverterBindingLocalDiagnosticExportReportINSTANCE = FfiConverterBindingLocalDiagnosticExportReport{}

func (c FfiConverterBindingLocalDiagnosticExportReport) Lift(rb RustBufferI) BindingLocalDiagnosticExportReport {
	return LiftFromRustBuffer[BindingLocalDiagnosticExportReport](c, rb)
}

func (c FfiConverterBindingLocalDiagnosticExportReport) Read(reader io.Reader) BindingLocalDiagnosticExportReport {
	return BindingLocalDiagnosticExportReport{
		FfiConverterBindingObservabilitySignalResultINSTANCE.Read(reader),
		FfiConverterBindingLocalDiagnosticStatusINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterSequenceBindingFileSourceCountsINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingLocalDiagnosticExportReport) Lower(value BindingLocalDiagnosticExportReport) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLocalDiagnosticExportReport](c, value)
}

func (c FfiConverterBindingLocalDiagnosticExportReport) LowerExternal(value BindingLocalDiagnosticExportReport) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLocalDiagnosticExportReport](c, value))
}

func (c FfiConverterBindingLocalDiagnosticExportReport) Write(writer io.Writer, value BindingLocalDiagnosticExportReport) {
	FfiConverterBindingObservabilitySignalResultINSTANCE.Write(writer, value.Flush)
	FfiConverterBindingLocalDiagnosticStatusINSTANCE.Write(writer, value.Status)
	FfiConverterStringINSTANCE.Write(writer, value.RequestedAtUtc)
	FfiConverterStringINSTANCE.Write(writer, value.CompletedAtUtc)
	FfiConverterBoolINSTANCE.Write(writer, value.OtherProcessesFlushed)
	FfiConverterSequenceBindingFileSourceCountsINSTANCE.Write(writer, value.Files)
}

type FfiDestroyerBindingLocalDiagnosticExportReport struct{}

func (_ FfiDestroyerBindingLocalDiagnosticExportReport) Destroy(value BindingLocalDiagnosticExportReport) {
	value.Destroy()
}

type BindingLocalDiagnosticStatus struct {
	RunId                     string
	Capture                   BindingLocalCaptureStatus
	ObservedRecords           uint64
	PolicyFilteredRecords     uint64
	SchemaRejectedRecords     uint64
	CorrelationLimitedRecords uint64
	EngineVersion             string
	SourceCommit              string
	CounterScope              string
	Sources                   []BindingSourceCoverage
	LocalFile                 BindingObservabilitySetupStatus
	Closed                    bool
}

func (r *BindingLocalDiagnosticStatus) Destroy() {
	FfiDestroyerString{}.Destroy(r.RunId)
	FfiDestroyerBindingLocalCaptureStatus{}.Destroy(r.Capture)
	FfiDestroyerUint64{}.Destroy(r.ObservedRecords)
	FfiDestroyerUint64{}.Destroy(r.PolicyFilteredRecords)
	FfiDestroyerUint64{}.Destroy(r.SchemaRejectedRecords)
	FfiDestroyerUint64{}.Destroy(r.CorrelationLimitedRecords)
	FfiDestroyerString{}.Destroy(r.EngineVersion)
	FfiDestroyerString{}.Destroy(r.SourceCommit)
	FfiDestroyerString{}.Destroy(r.CounterScope)
	FfiDestroyerSequenceBindingSourceCoverage{}.Destroy(r.Sources)
	FfiDestroyerBindingObservabilitySetupStatus{}.Destroy(r.LocalFile)
	FfiDestroyerBool{}.Destroy(r.Closed)
}

type FfiConverterBindingLocalDiagnosticStatus struct{}

var FfiConverterBindingLocalDiagnosticStatusINSTANCE = FfiConverterBindingLocalDiagnosticStatus{}

func (c FfiConverterBindingLocalDiagnosticStatus) Lift(rb RustBufferI) BindingLocalDiagnosticStatus {
	return LiftFromRustBuffer[BindingLocalDiagnosticStatus](c, rb)
}

func (c FfiConverterBindingLocalDiagnosticStatus) Read(reader io.Reader) BindingLocalDiagnosticStatus {
	return BindingLocalDiagnosticStatus{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBindingLocalCaptureStatusINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterSequenceBindingSourceCoverageINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySetupStatusINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingLocalDiagnosticStatus) Lower(value BindingLocalDiagnosticStatus) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLocalDiagnosticStatus](c, value)
}

func (c FfiConverterBindingLocalDiagnosticStatus) LowerExternal(value BindingLocalDiagnosticStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLocalDiagnosticStatus](c, value))
}

func (c FfiConverterBindingLocalDiagnosticStatus) Write(writer io.Writer, value BindingLocalDiagnosticStatus) {
	FfiConverterStringINSTANCE.Write(writer, value.RunId)
	FfiConverterBindingLocalCaptureStatusINSTANCE.Write(writer, value.Capture)
	FfiConverterUint64INSTANCE.Write(writer, value.ObservedRecords)
	FfiConverterUint64INSTANCE.Write(writer, value.PolicyFilteredRecords)
	FfiConverterUint64INSTANCE.Write(writer, value.SchemaRejectedRecords)
	FfiConverterUint64INSTANCE.Write(writer, value.CorrelationLimitedRecords)
	FfiConverterStringINSTANCE.Write(writer, value.EngineVersion)
	FfiConverterStringINSTANCE.Write(writer, value.SourceCommit)
	FfiConverterStringINSTANCE.Write(writer, value.CounterScope)
	FfiConverterSequenceBindingSourceCoverageINSTANCE.Write(writer, value.Sources)
	FfiConverterBindingObservabilitySetupStatusINSTANCE.Write(writer, value.LocalFile)
	FfiConverterBoolINSTANCE.Write(writer, value.Closed)
}

type FfiDestroyerBindingLocalDiagnosticStatus struct{}

func (_ FfiDestroyerBindingLocalDiagnosticStatus) Destroy(value BindingLocalDiagnosticStatus) {
	value.Destroy()
}

type BindingObservabilityConfig struct {
	ServiceVersion           string
	Environment              BindingDeploymentEnvironment
	AppChannel               string
	RemoteDiagnosticsEnabled bool
	Collector                *BindingCollectorConfig
}

func (r *BindingObservabilityConfig) Destroy() {
	FfiDestroyerString{}.Destroy(r.ServiceVersion)
	FfiDestroyerBindingDeploymentEnvironment{}.Destroy(r.Environment)
	FfiDestroyerString{}.Destroy(r.AppChannel)
	FfiDestroyerBool{}.Destroy(r.RemoteDiagnosticsEnabled)
	FfiDestroyerOptionalBindingCollectorConfig{}.Destroy(r.Collector)
}

type FfiConverterBindingObservabilityConfig struct{}

var FfiConverterBindingObservabilityConfigINSTANCE = FfiConverterBindingObservabilityConfig{}

func (c FfiConverterBindingObservabilityConfig) Lift(rb RustBufferI) BindingObservabilityConfig {
	return LiftFromRustBuffer[BindingObservabilityConfig](c, rb)
}

func (c FfiConverterBindingObservabilityConfig) Read(reader io.Reader) BindingObservabilityConfig {
	return BindingObservabilityConfig{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBindingDeploymentEnvironmentINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterOptionalBindingCollectorConfigINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingObservabilityConfig) Lower(value BindingObservabilityConfig) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilityConfig](c, value)
}

func (c FfiConverterBindingObservabilityConfig) LowerExternal(value BindingObservabilityConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilityConfig](c, value))
}

func (c FfiConverterBindingObservabilityConfig) Write(writer io.Writer, value BindingObservabilityConfig) {
	FfiConverterStringINSTANCE.Write(writer, value.ServiceVersion)
	FfiConverterBindingDeploymentEnvironmentINSTANCE.Write(writer, value.Environment)
	FfiConverterStringINSTANCE.Write(writer, value.AppChannel)
	FfiConverterBoolINSTANCE.Write(writer, value.RemoteDiagnosticsEnabled)
	FfiConverterOptionalBindingCollectorConfigINSTANCE.Write(writer, value.Collector)
}

type FfiDestroyerBindingObservabilityConfig struct{}

func (_ FfiDestroyerBindingObservabilityConfig) Destroy(value BindingObservabilityConfig) {
	value.Destroy()
}

type BindingObservabilityFlushSummary struct {
	Traces BindingObservabilitySignalResult
	Logs   BindingObservabilitySignalResult
}

func (r *BindingObservabilityFlushSummary) Destroy() {
	FfiDestroyerBindingObservabilitySignalResult{}.Destroy(r.Traces)
	FfiDestroyerBindingObservabilitySignalResult{}.Destroy(r.Logs)
}

type FfiConverterBindingObservabilityFlushSummary struct{}

var FfiConverterBindingObservabilityFlushSummaryINSTANCE = FfiConverterBindingObservabilityFlushSummary{}

func (c FfiConverterBindingObservabilityFlushSummary) Lift(rb RustBufferI) BindingObservabilityFlushSummary {
	return LiftFromRustBuffer[BindingObservabilityFlushSummary](c, rb)
}

func (c FfiConverterBindingObservabilityFlushSummary) Read(reader io.Reader) BindingObservabilityFlushSummary {
	return BindingObservabilityFlushSummary{
		FfiConverterBindingObservabilitySignalResultINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySignalResultINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingObservabilityFlushSummary) Lower(value BindingObservabilityFlushSummary) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilityFlushSummary](c, value)
}

func (c FfiConverterBindingObservabilityFlushSummary) LowerExternal(value BindingObservabilityFlushSummary) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilityFlushSummary](c, value))
}

func (c FfiConverterBindingObservabilityFlushSummary) Write(writer io.Writer, value BindingObservabilityFlushSummary) {
	FfiConverterBindingObservabilitySignalResultINSTANCE.Write(writer, value.Traces)
	FfiConverterBindingObservabilitySignalResultINSTANCE.Write(writer, value.Logs)
}

type FfiDestroyerBindingObservabilityFlushSummary struct{}

func (_ FfiDestroyerBindingObservabilityFlushSummary) Destroy(value BindingObservabilityFlushSummary) {
	value.Destroy()
}

type BindingObservabilityHealth struct {
	Remote                  BindingObservabilitySetupStatus
	RemoteSetupFailure      *BindingObservabilityRemoteSetupFailure
	LocalFile               BindingObservabilitySetupStatus
	DroppedLocalRecords     uint64
	DroppedRemoteSpans      uint64
	DroppedRemoteLogs       uint64
	FailedRemoteSpanBatches uint64
	FailedRemoteLogBatches  uint64
}

func (r *BindingObservabilityHealth) Destroy() {
	FfiDestroyerBindingObservabilitySetupStatus{}.Destroy(r.Remote)
	FfiDestroyerOptionalBindingObservabilityRemoteSetupFailure{}.Destroy(r.RemoteSetupFailure)
	FfiDestroyerBindingObservabilitySetupStatus{}.Destroy(r.LocalFile)
	FfiDestroyerUint64{}.Destroy(r.DroppedLocalRecords)
	FfiDestroyerUint64{}.Destroy(r.DroppedRemoteSpans)
	FfiDestroyerUint64{}.Destroy(r.DroppedRemoteLogs)
	FfiDestroyerUint64{}.Destroy(r.FailedRemoteSpanBatches)
	FfiDestroyerUint64{}.Destroy(r.FailedRemoteLogBatches)
}

type FfiConverterBindingObservabilityHealth struct{}

var FfiConverterBindingObservabilityHealthINSTANCE = FfiConverterBindingObservabilityHealth{}

func (c FfiConverterBindingObservabilityHealth) Lift(rb RustBufferI) BindingObservabilityHealth {
	return LiftFromRustBuffer[BindingObservabilityHealth](c, rb)
}

func (c FfiConverterBindingObservabilityHealth) Read(reader io.Reader) BindingObservabilityHealth {
	return BindingObservabilityHealth{
		FfiConverterBindingObservabilitySetupStatusINSTANCE.Read(reader),
		FfiConverterOptionalBindingObservabilityRemoteSetupFailureINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySetupStatusINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingObservabilityHealth) Lower(value BindingObservabilityHealth) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilityHealth](c, value)
}

func (c FfiConverterBindingObservabilityHealth) LowerExternal(value BindingObservabilityHealth) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilityHealth](c, value))
}

func (c FfiConverterBindingObservabilityHealth) Write(writer io.Writer, value BindingObservabilityHealth) {
	FfiConverterBindingObservabilitySetupStatusINSTANCE.Write(writer, value.Remote)
	FfiConverterOptionalBindingObservabilityRemoteSetupFailureINSTANCE.Write(writer, value.RemoteSetupFailure)
	FfiConverterBindingObservabilitySetupStatusINSTANCE.Write(writer, value.LocalFile)
	FfiConverterUint64INSTANCE.Write(writer, value.DroppedLocalRecords)
	FfiConverterUint64INSTANCE.Write(writer, value.DroppedRemoteSpans)
	FfiConverterUint64INSTANCE.Write(writer, value.DroppedRemoteLogs)
	FfiConverterUint64INSTANCE.Write(writer, value.FailedRemoteSpanBatches)
	FfiConverterUint64INSTANCE.Write(writer, value.FailedRemoteLogBatches)
}

type FfiDestroyerBindingObservabilityHealth struct{}

func (_ FfiDestroyerBindingObservabilityHealth) Destroy(value BindingObservabilityHealth) {
	value.Destroy()
}

type BindingObservabilitySetup struct {
	Reused              bool
	Remote              BindingObservabilitySetupStatus
	LocalFile           BindingObservabilitySetupStatus
	DroppedLocalRecords uint64
}

func (r *BindingObservabilitySetup) Destroy() {
	FfiDestroyerBool{}.Destroy(r.Reused)
	FfiDestroyerBindingObservabilitySetupStatus{}.Destroy(r.Remote)
	FfiDestroyerBindingObservabilitySetupStatus{}.Destroy(r.LocalFile)
	FfiDestroyerUint64{}.Destroy(r.DroppedLocalRecords)
}

type FfiConverterBindingObservabilitySetup struct{}

var FfiConverterBindingObservabilitySetupINSTANCE = FfiConverterBindingObservabilitySetup{}

func (c FfiConverterBindingObservabilitySetup) Lift(rb RustBufferI) BindingObservabilitySetup {
	return LiftFromRustBuffer[BindingObservabilitySetup](c, rb)
}

func (c FfiConverterBindingObservabilitySetup) Read(reader io.Reader) BindingObservabilitySetup {
	return BindingObservabilitySetup{
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySetupStatusINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySetupStatusINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingObservabilitySetup) Lower(value BindingObservabilitySetup) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilitySetup](c, value)
}

func (c FfiConverterBindingObservabilitySetup) LowerExternal(value BindingObservabilitySetup) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilitySetup](c, value))
}

func (c FfiConverterBindingObservabilitySetup) Write(writer io.Writer, value BindingObservabilitySetup) {
	FfiConverterBoolINSTANCE.Write(writer, value.Reused)
	FfiConverterBindingObservabilitySetupStatusINSTANCE.Write(writer, value.Remote)
	FfiConverterBindingObservabilitySetupStatusINSTANCE.Write(writer, value.LocalFile)
	FfiConverterUint64INSTANCE.Write(writer, value.DroppedLocalRecords)
}

type FfiDestroyerBindingObservabilitySetup struct{}

func (_ FfiDestroyerBindingObservabilitySetup) Destroy(value BindingObservabilitySetup) {
	value.Destroy()
}

type BindingObservabilityShutdownSummary struct {
	Traces BindingObservabilitySignalResult
	Logs   BindingObservabilitySignalResult
}

func (r *BindingObservabilityShutdownSummary) Destroy() {
	FfiDestroyerBindingObservabilitySignalResult{}.Destroy(r.Traces)
	FfiDestroyerBindingObservabilitySignalResult{}.Destroy(r.Logs)
}

type FfiConverterBindingObservabilityShutdownSummary struct{}

var FfiConverterBindingObservabilityShutdownSummaryINSTANCE = FfiConverterBindingObservabilityShutdownSummary{}

func (c FfiConverterBindingObservabilityShutdownSummary) Lift(rb RustBufferI) BindingObservabilityShutdownSummary {
	return LiftFromRustBuffer[BindingObservabilityShutdownSummary](c, rb)
}

func (c FfiConverterBindingObservabilityShutdownSummary) Read(reader io.Reader) BindingObservabilityShutdownSummary {
	return BindingObservabilityShutdownSummary{
		FfiConverterBindingObservabilitySignalResultINSTANCE.Read(reader),
		FfiConverterBindingObservabilitySignalResultINSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingObservabilityShutdownSummary) Lower(value BindingObservabilityShutdownSummary) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilityShutdownSummary](c, value)
}

func (c FfiConverterBindingObservabilityShutdownSummary) LowerExternal(value BindingObservabilityShutdownSummary) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilityShutdownSummary](c, value))
}

func (c FfiConverterBindingObservabilityShutdownSummary) Write(writer io.Writer, value BindingObservabilityShutdownSummary) {
	FfiConverterBindingObservabilitySignalResultINSTANCE.Write(writer, value.Traces)
	FfiConverterBindingObservabilitySignalResultINSTANCE.Write(writer, value.Logs)
}

type FfiDestroyerBindingObservabilityShutdownSummary struct{}

func (_ FfiDestroyerBindingObservabilityShutdownSummary) Destroy(value BindingObservabilityShutdownSummary) {
	value.Destroy()
}

type BindingSourceCoverage struct {
	Source              BindingLocalDiagnosticSource
	Capability          BindingSourceCapability
	Collection          BindingSourceCollection
	ObservedCount       uint64
	PolicyFilteredCount uint64
}

func (r *BindingSourceCoverage) Destroy() {
	FfiDestroyerBindingLocalDiagnosticSource{}.Destroy(r.Source)
	FfiDestroyerBindingSourceCapability{}.Destroy(r.Capability)
	FfiDestroyerBindingSourceCollection{}.Destroy(r.Collection)
	FfiDestroyerUint64{}.Destroy(r.ObservedCount)
	FfiDestroyerUint64{}.Destroy(r.PolicyFilteredCount)
}

type FfiConverterBindingSourceCoverage struct{}

var FfiConverterBindingSourceCoverageINSTANCE = FfiConverterBindingSourceCoverage{}

func (c FfiConverterBindingSourceCoverage) Lift(rb RustBufferI) BindingSourceCoverage {
	return LiftFromRustBuffer[BindingSourceCoverage](c, rb)
}

func (c FfiConverterBindingSourceCoverage) Read(reader io.Reader) BindingSourceCoverage {
	return BindingSourceCoverage{
		FfiConverterBindingLocalDiagnosticSourceINSTANCE.Read(reader),
		FfiConverterBindingSourceCapabilityINSTANCE.Read(reader),
		FfiConverterBindingSourceCollectionINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterBindingSourceCoverage) Lower(value BindingSourceCoverage) C.RustBuffer {
	return LowerIntoRustBuffer[BindingSourceCoverage](c, value)
}

func (c FfiConverterBindingSourceCoverage) LowerExternal(value BindingSourceCoverage) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingSourceCoverage](c, value))
}

func (c FfiConverterBindingSourceCoverage) Write(writer io.Writer, value BindingSourceCoverage) {
	FfiConverterBindingLocalDiagnosticSourceINSTANCE.Write(writer, value.Source)
	FfiConverterBindingSourceCapabilityINSTANCE.Write(writer, value.Capability)
	FfiConverterBindingSourceCollectionINSTANCE.Write(writer, value.Collection)
	FfiConverterUint64INSTANCE.Write(writer, value.ObservedCount)
	FfiConverterUint64INSTANCE.Write(writer, value.PolicyFilteredCount)
}

type FfiDestroyerBindingSourceCoverage struct{}

func (_ FfiDestroyerBindingSourceCoverage) Destroy(value BindingSourceCoverage) {
	value.Destroy()
}

type CustomRelay struct {
	Url                  string
	CredentialConfigured bool
}

func (r *CustomRelay) Destroy() {
	FfiDestroyerString{}.Destroy(r.Url)
	FfiDestroyerBool{}.Destroy(r.CredentialConfigured)
}

type FfiConverterCustomRelay struct{}

var FfiConverterCustomRelayINSTANCE = FfiConverterCustomRelay{}

func (c FfiConverterCustomRelay) Lift(rb RustBufferI) CustomRelay {
	return LiftFromRustBuffer[CustomRelay](c, rb)
}

func (c FfiConverterCustomRelay) Read(reader io.Reader) CustomRelay {
	return CustomRelay{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterCustomRelay) Lower(value CustomRelay) C.RustBuffer {
	return LowerIntoRustBuffer[CustomRelay](c, value)
}

func (c FfiConverterCustomRelay) LowerExternal(value CustomRelay) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CustomRelay](c, value))
}

func (c FfiConverterCustomRelay) Write(writer io.Writer, value CustomRelay) {
	FfiConverterStringINSTANCE.Write(writer, value.Url)
	FfiConverterBoolINSTANCE.Write(writer, value.CredentialConfigured)
}

type FfiDestroyerCustomRelay struct{}

func (_ FfiDestroyerCustomRelay) Destroy(value CustomRelay) {
	value.Destroy()
}

type CustomRelayMutationResult struct {
	Relays    []CustomRelay
	Rejection *CustomRelayMutationRejection
}

func (r *CustomRelayMutationResult) Destroy() {
	FfiDestroyerSequenceCustomRelay{}.Destroy(r.Relays)
	FfiDestroyerOptionalCustomRelayMutationRejection{}.Destroy(r.Rejection)
}

type FfiConverterCustomRelayMutationResult struct{}

var FfiConverterCustomRelayMutationResultINSTANCE = FfiConverterCustomRelayMutationResult{}

func (c FfiConverterCustomRelayMutationResult) Lift(rb RustBufferI) CustomRelayMutationResult {
	return LiftFromRustBuffer[CustomRelayMutationResult](c, rb)
}

func (c FfiConverterCustomRelayMutationResult) Read(reader io.Reader) CustomRelayMutationResult {
	return CustomRelayMutationResult{
		FfiConverterSequenceCustomRelayINSTANCE.Read(reader),
		FfiConverterOptionalCustomRelayMutationRejectionINSTANCE.Read(reader),
	}
}

func (c FfiConverterCustomRelayMutationResult) Lower(value CustomRelayMutationResult) C.RustBuffer {
	return LowerIntoRustBuffer[CustomRelayMutationResult](c, value)
}

func (c FfiConverterCustomRelayMutationResult) LowerExternal(value CustomRelayMutationResult) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CustomRelayMutationResult](c, value))
}

func (c FfiConverterCustomRelayMutationResult) Write(writer io.Writer, value CustomRelayMutationResult) {
	FfiConverterSequenceCustomRelayINSTANCE.Write(writer, value.Relays)
	FfiConverterOptionalCustomRelayMutationRejectionINSTANCE.Write(writer, value.Rejection)
}

type FfiDestroyerCustomRelayMutationResult struct{}

func (_ FfiDestroyerCustomRelayMutationResult) Destroy(value CustomRelayMutationResult) {
	value.Destroy()
}

type Device struct {
	DeviceId    string
	DisplayName string
	IsLocal     bool
	Online      bool
}

func (r *Device) Destroy() {
	FfiDestroyerString{}.Destroy(r.DeviceId)
	FfiDestroyerString{}.Destroy(r.DisplayName)
	FfiDestroyerBool{}.Destroy(r.IsLocal)
	FfiDestroyerBool{}.Destroy(r.Online)
}

type FfiConverterDevice struct{}

var FfiConverterDeviceINSTANCE = FfiConverterDevice{}

func (c FfiConverterDevice) Lift(rb RustBufferI) Device {
	return LiftFromRustBuffer[Device](c, rb)
}

func (c FfiConverterDevice) Read(reader io.Reader) Device {
	return Device{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterDevice) Lower(value Device) C.RustBuffer {
	return LowerIntoRustBuffer[Device](c, value)
}

func (c FfiConverterDevice) LowerExternal(value Device) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[Device](c, value))
}

func (c FfiConverterDevice) Write(writer io.Writer, value Device) {
	FfiConverterStringINSTANCE.Write(writer, value.DeviceId)
	FfiConverterStringINSTANCE.Write(writer, value.DisplayName)
	FfiConverterBoolINSTANCE.Write(writer, value.IsLocal)
	FfiConverterBoolINSTANCE.Write(writer, value.Online)
}

type FfiDestroyerDevice struct{}

func (_ FfiDestroyerDevice) Destroy(value Device) {
	value.Destroy()
}

type InvitationIssued struct {
	InvitationCode string
	FullInvitation string
	ExpiresAtMs    int64
	Availability   InvitationAvailability
}

func (r *InvitationIssued) Destroy() {
	FfiDestroyerString{}.Destroy(r.InvitationCode)
	FfiDestroyerString{}.Destroy(r.FullInvitation)
	FfiDestroyerInt64{}.Destroy(r.ExpiresAtMs)
	FfiDestroyerInvitationAvailability{}.Destroy(r.Availability)
}

type FfiConverterInvitationIssued struct{}

var FfiConverterInvitationIssuedINSTANCE = FfiConverterInvitationIssued{}

func (c FfiConverterInvitationIssued) Lift(rb RustBufferI) InvitationIssued {
	return LiftFromRustBuffer[InvitationIssued](c, rb)
}

func (c FfiConverterInvitationIssued) Read(reader io.Reader) InvitationIssued {
	return InvitationIssued{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterInt64INSTANCE.Read(reader),
		FfiConverterInvitationAvailabilityINSTANCE.Read(reader),
	}
}

func (c FfiConverterInvitationIssued) Lower(value InvitationIssued) C.RustBuffer {
	return LowerIntoRustBuffer[InvitationIssued](c, value)
}

func (c FfiConverterInvitationIssued) LowerExternal(value InvitationIssued) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[InvitationIssued](c, value))
}

func (c FfiConverterInvitationIssued) Write(writer io.Writer, value InvitationIssued) {
	FfiConverterStringINSTANCE.Write(writer, value.InvitationCode)
	FfiConverterStringINSTANCE.Write(writer, value.FullInvitation)
	FfiConverterInt64INSTANCE.Write(writer, value.ExpiresAtMs)
	FfiConverterInvitationAvailabilityINSTANCE.Write(writer, value.Availability)
}

type FfiDestroyerInvitationIssued struct{}

func (_ FfiDestroyerInvitationIssued) Destroy(value InvitationIssued) {
	value.Destroy()
}

type JoinedSpace struct {
	SponsorDeviceId            string
	SponsorIdentityFingerprint string
	SpaceId                    string
	SelfDeviceId               string
	SelfIdentityFingerprint    string
	MigratedRecords            *uint64
	PreservedUnreadableRecords *uint64
}

func (r *JoinedSpace) Destroy() {
	FfiDestroyerString{}.Destroy(r.SponsorDeviceId)
	FfiDestroyerString{}.Destroy(r.SponsorIdentityFingerprint)
	FfiDestroyerString{}.Destroy(r.SpaceId)
	FfiDestroyerString{}.Destroy(r.SelfDeviceId)
	FfiDestroyerString{}.Destroy(r.SelfIdentityFingerprint)
	FfiDestroyerOptionalUint64{}.Destroy(r.MigratedRecords)
	FfiDestroyerOptionalUint64{}.Destroy(r.PreservedUnreadableRecords)
}

type FfiConverterJoinedSpace struct{}

var FfiConverterJoinedSpaceINSTANCE = FfiConverterJoinedSpace{}

func (c FfiConverterJoinedSpace) Lift(rb RustBufferI) JoinedSpace {
	return LiftFromRustBuffer[JoinedSpace](c, rb)
}

func (c FfiConverterJoinedSpace) Read(reader io.Reader) JoinedSpace {
	return JoinedSpace{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalUint64INSTANCE.Read(reader),
		FfiConverterOptionalUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterJoinedSpace) Lower(value JoinedSpace) C.RustBuffer {
	return LowerIntoRustBuffer[JoinedSpace](c, value)
}

func (c FfiConverterJoinedSpace) LowerExternal(value JoinedSpace) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinedSpace](c, value))
}

func (c FfiConverterJoinedSpace) Write(writer io.Writer, value JoinedSpace) {
	FfiConverterStringINSTANCE.Write(writer, value.SponsorDeviceId)
	FfiConverterStringINSTANCE.Write(writer, value.SponsorIdentityFingerprint)
	FfiConverterStringINSTANCE.Write(writer, value.SpaceId)
	FfiConverterStringINSTANCE.Write(writer, value.SelfDeviceId)
	FfiConverterStringINSTANCE.Write(writer, value.SelfIdentityFingerprint)
	FfiConverterOptionalUint64INSTANCE.Write(writer, value.MigratedRecords)
	FfiConverterOptionalUint64INSTANCE.Write(writer, value.PreservedUnreadableRecords)
}

type FfiDestroyerJoinedSpace struct{}

func (_ FfiDestroyerJoinedSpace) Destroy(value JoinedSpace) {
	value.Destroy()
}

type LocalDevice struct {
	DeviceId    string
	DisplayName string
}

func (r *LocalDevice) Destroy() {
	FfiDestroyerString{}.Destroy(r.DeviceId)
	FfiDestroyerString{}.Destroy(r.DisplayName)
}

type FfiConverterLocalDevice struct{}

var FfiConverterLocalDeviceINSTANCE = FfiConverterLocalDevice{}

func (c FfiConverterLocalDevice) Lift(rb RustBufferI) LocalDevice {
	return LiftFromRustBuffer[LocalDevice](c, rb)
}

func (c FfiConverterLocalDevice) Read(reader io.Reader) LocalDevice {
	return LocalDevice{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterLocalDevice) Lower(value LocalDevice) C.RustBuffer {
	return LowerIntoRustBuffer[LocalDevice](c, value)
}

func (c FfiConverterLocalDevice) LowerExternal(value LocalDevice) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[LocalDevice](c, value))
}

func (c FfiConverterLocalDevice) Write(writer io.Writer, value LocalDevice) {
	FfiConverterStringINSTANCE.Write(writer, value.DeviceId)
	FfiConverterStringINSTANCE.Write(writer, value.DisplayName)
}

type FfiDestroyerLocalDevice struct{}

func (_ FfiDestroyerLocalDevice) Destroy(value LocalDevice) {
	value.Destroy()
}

type NetworkRecoveryStatus struct {
	Phase         string
	Retryable     bool
	NextRetryInMs *uint64
}

func (r *NetworkRecoveryStatus) Destroy() {
	FfiDestroyerString{}.Destroy(r.Phase)
	FfiDestroyerBool{}.Destroy(r.Retryable)
	FfiDestroyerOptionalUint64{}.Destroy(r.NextRetryInMs)
}

type FfiConverterNetworkRecoveryStatus struct{}

var FfiConverterNetworkRecoveryStatusINSTANCE = FfiConverterNetworkRecoveryStatus{}

func (c FfiConverterNetworkRecoveryStatus) Lift(rb RustBufferI) NetworkRecoveryStatus {
	return LiftFromRustBuffer[NetworkRecoveryStatus](c, rb)
}

func (c FfiConverterNetworkRecoveryStatus) Read(reader io.Reader) NetworkRecoveryStatus {
	return NetworkRecoveryStatus{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterOptionalUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterNetworkRecoveryStatus) Lower(value NetworkRecoveryStatus) C.RustBuffer {
	return LowerIntoRustBuffer[NetworkRecoveryStatus](c, value)
}

func (c FfiConverterNetworkRecoveryStatus) LowerExternal(value NetworkRecoveryStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[NetworkRecoveryStatus](c, value))
}

func (c FfiConverterNetworkRecoveryStatus) Write(writer io.Writer, value NetworkRecoveryStatus) {
	FfiConverterStringINSTANCE.Write(writer, value.Phase)
	FfiConverterBoolINSTANCE.Write(writer, value.Retryable)
	FfiConverterOptionalUint64INSTANCE.Write(writer, value.NextRetryInMs)
}

type FfiDestroyerNetworkRecoveryStatus struct{}

func (_ FfiDestroyerNetworkRecoveryStatus) Destroy(value NetworkRecoveryStatus) {
	value.Destroy()
}

type PeerConnectionRefresh struct {
	Total   uint64
	Online  uint64
	Offline uint64
	Errors  uint64
}

func (r *PeerConnectionRefresh) Destroy() {
	FfiDestroyerUint64{}.Destroy(r.Total)
	FfiDestroyerUint64{}.Destroy(r.Online)
	FfiDestroyerUint64{}.Destroy(r.Offline)
	FfiDestroyerUint64{}.Destroy(r.Errors)
}

type FfiConverterPeerConnectionRefresh struct{}

var FfiConverterPeerConnectionRefreshINSTANCE = FfiConverterPeerConnectionRefresh{}

func (c FfiConverterPeerConnectionRefresh) Lift(rb RustBufferI) PeerConnectionRefresh {
	return LiftFromRustBuffer[PeerConnectionRefresh](c, rb)
}

func (c FfiConverterPeerConnectionRefresh) Read(reader io.Reader) PeerConnectionRefresh {
	return PeerConnectionRefresh{
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterPeerConnectionRefresh) Lower(value PeerConnectionRefresh) C.RustBuffer {
	return LowerIntoRustBuffer[PeerConnectionRefresh](c, value)
}

func (c FfiConverterPeerConnectionRefresh) LowerExternal(value PeerConnectionRefresh) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[PeerConnectionRefresh](c, value))
}

func (c FfiConverterPeerConnectionRefresh) Write(writer io.Writer, value PeerConnectionRefresh) {
	FfiConverterUint64INSTANCE.Write(writer, value.Total)
	FfiConverterUint64INSTANCE.Write(writer, value.Online)
	FfiConverterUint64INSTANCE.Write(writer, value.Offline)
	FfiConverterUint64INSTANCE.Write(writer, value.Errors)
}

type FfiDestroyerPeerConnectionRefresh struct{}

func (_ FfiDestroyerPeerConnectionRefresh) Destroy(value PeerConnectionRefresh) {
	value.Destroy()
}

type RelayOverview struct {
	SavedMode     RelayRoutingMode
	AppliedMode   *RelayRoutingMode
	ChangePending bool
	Entries       []RelayOverviewEntry
}

func (r *RelayOverview) Destroy() {
	FfiDestroyerRelayRoutingMode{}.Destroy(r.SavedMode)
	FfiDestroyerOptionalRelayRoutingMode{}.Destroy(r.AppliedMode)
	FfiDestroyerBool{}.Destroy(r.ChangePending)
	FfiDestroyerSequenceRelayOverviewEntry{}.Destroy(r.Entries)
}

type FfiConverterRelayOverview struct{}

var FfiConverterRelayOverviewINSTANCE = FfiConverterRelayOverview{}

func (c FfiConverterRelayOverview) Lift(rb RustBufferI) RelayOverview {
	return LiftFromRustBuffer[RelayOverview](c, rb)
}

func (c FfiConverterRelayOverview) Read(reader io.Reader) RelayOverview {
	return RelayOverview{
		FfiConverterRelayRoutingModeINSTANCE.Read(reader),
		FfiConverterOptionalRelayRoutingModeINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterSequenceRelayOverviewEntryINSTANCE.Read(reader),
	}
}

func (c FfiConverterRelayOverview) Lower(value RelayOverview) C.RustBuffer {
	return LowerIntoRustBuffer[RelayOverview](c, value)
}

func (c FfiConverterRelayOverview) LowerExternal(value RelayOverview) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RelayOverview](c, value))
}

func (c FfiConverterRelayOverview) Write(writer io.Writer, value RelayOverview) {
	FfiConverterRelayRoutingModeINSTANCE.Write(writer, value.SavedMode)
	FfiConverterOptionalRelayRoutingModeINSTANCE.Write(writer, value.AppliedMode)
	FfiConverterBoolINSTANCE.Write(writer, value.ChangePending)
	FfiConverterSequenceRelayOverviewEntryINSTANCE.Write(writer, value.Entries)
}

type FfiDestroyerRelayOverview struct{}

func (_ FfiDestroyerRelayOverview) Destroy(value RelayOverview) {
	value.Destroy()
}

// `in_effect` 表示运行中的节点按此地址配置，不代表已经连通。
type RelayOverviewEntry struct {
	Source               RelayEntrySource
	RegionId             *string
	Url                  string
	CredentialConfigured bool
	InEffect             bool
}

func (r *RelayOverviewEntry) Destroy() {
	FfiDestroyerRelayEntrySource{}.Destroy(r.Source)
	FfiDestroyerOptionalString{}.Destroy(r.RegionId)
	FfiDestroyerString{}.Destroy(r.Url)
	FfiDestroyerBool{}.Destroy(r.CredentialConfigured)
	FfiDestroyerBool{}.Destroy(r.InEffect)
}

type FfiConverterRelayOverviewEntry struct{}

var FfiConverterRelayOverviewEntryINSTANCE = FfiConverterRelayOverviewEntry{}

func (c FfiConverterRelayOverviewEntry) Lift(rb RustBufferI) RelayOverviewEntry {
	return LiftFromRustBuffer[RelayOverviewEntry](c, rb)
}

func (c FfiConverterRelayOverviewEntry) Read(reader io.Reader) RelayOverviewEntry {
	return RelayOverviewEntry{
		FfiConverterRelayEntrySourceINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterRelayOverviewEntry) Lower(value RelayOverviewEntry) C.RustBuffer {
	return LowerIntoRustBuffer[RelayOverviewEntry](c, value)
}

func (c FfiConverterRelayOverviewEntry) LowerExternal(value RelayOverviewEntry) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RelayOverviewEntry](c, value))
}

func (c FfiConverterRelayOverviewEntry) Write(writer io.Writer, value RelayOverviewEntry) {
	FfiConverterRelayEntrySourceINSTANCE.Write(writer, value.Source)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.RegionId)
	FfiConverterStringINSTANCE.Write(writer, value.Url)
	FfiConverterBoolINSTANCE.Write(writer, value.CredentialConfigured)
	FfiConverterBoolINSTANCE.Write(writer, value.InEffect)
}

type FfiDestroyerRelayOverviewEntry struct{}

func (_ FfiDestroyerRelayOverviewEntry) Destroy(value RelayOverviewEntry) {
	value.Destroy()
}

type RelaySaveResult struct {
	Configured bool
}

func (r *RelaySaveResult) Destroy() {
	FfiDestroyerBool{}.Destroy(r.Configured)
}

type FfiConverterRelaySaveResult struct{}

var FfiConverterRelaySaveResultINSTANCE = FfiConverterRelaySaveResult{}

func (c FfiConverterRelaySaveResult) Lift(rb RustBufferI) RelaySaveResult {
	return LiftFromRustBuffer[RelaySaveResult](c, rb)
}

func (c FfiConverterRelaySaveResult) Read(reader io.Reader) RelaySaveResult {
	return RelaySaveResult{
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterRelaySaveResult) Lower(value RelaySaveResult) C.RustBuffer {
	return LowerIntoRustBuffer[RelaySaveResult](c, value)
}

func (c FfiConverterRelaySaveResult) LowerExternal(value RelaySaveResult) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RelaySaveResult](c, value))
}

func (c FfiConverterRelaySaveResult) Write(writer io.Writer, value RelaySaveResult) {
	FfiConverterBoolINSTANCE.Write(writer, value.Configured)
}

type FfiDestroyerRelaySaveResult struct{}

func (_ FfiDestroyerRelaySaveResult) Destroy(value RelaySaveResult) {
	value.Destroy()
}

type SendReport struct {
	EntryId        string
	AtMs           int64
	TotalAccepted  uint64
	TotalDuplicate uint64
	TotalOffline   uint64
	TotalErrored   uint64
	TotalPending   uint64
}

func (r *SendReport) Destroy() {
	FfiDestroyerString{}.Destroy(r.EntryId)
	FfiDestroyerInt64{}.Destroy(r.AtMs)
	FfiDestroyerUint64{}.Destroy(r.TotalAccepted)
	FfiDestroyerUint64{}.Destroy(r.TotalDuplicate)
	FfiDestroyerUint64{}.Destroy(r.TotalOffline)
	FfiDestroyerUint64{}.Destroy(r.TotalErrored)
	FfiDestroyerUint64{}.Destroy(r.TotalPending)
}

type FfiConverterSendReport struct{}

var FfiConverterSendReportINSTANCE = FfiConverterSendReport{}

func (c FfiConverterSendReport) Lift(rb RustBufferI) SendReport {
	return LiftFromRustBuffer[SendReport](c, rb)
}

func (c FfiConverterSendReport) Read(reader io.Reader) SendReport {
	return SendReport{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterInt64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
	}
}

func (c FfiConverterSendReport) Lower(value SendReport) C.RustBuffer {
	return LowerIntoRustBuffer[SendReport](c, value)
}

func (c FfiConverterSendReport) LowerExternal(value SendReport) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SendReport](c, value))
}

func (c FfiConverterSendReport) Write(writer io.Writer, value SendReport) {
	FfiConverterStringINSTANCE.Write(writer, value.EntryId)
	FfiConverterInt64INSTANCE.Write(writer, value.AtMs)
	FfiConverterUint64INSTANCE.Write(writer, value.TotalAccepted)
	FfiConverterUint64INSTANCE.Write(writer, value.TotalDuplicate)
	FfiConverterUint64INSTANCE.Write(writer, value.TotalOffline)
	FfiConverterUint64INSTANCE.Write(writer, value.TotalErrored)
	FfiConverterUint64INSTANCE.Write(writer, value.TotalPending)
}

type FfiDestroyerSendReport struct{}

func (_ FfiDestroyerSendReport) Destroy(value SendReport) {
	value.Destroy()
}

type SessionRecovery struct {
	Unlocked bool
	Resumed  bool
}

func (r *SessionRecovery) Destroy() {
	FfiDestroyerBool{}.Destroy(r.Unlocked)
	FfiDestroyerBool{}.Destroy(r.Resumed)
}

type FfiConverterSessionRecovery struct{}

var FfiConverterSessionRecoveryINSTANCE = FfiConverterSessionRecovery{}

func (c FfiConverterSessionRecovery) Lift(rb RustBufferI) SessionRecovery {
	return LiftFromRustBuffer[SessionRecovery](c, rb)
}

func (c FfiConverterSessionRecovery) Read(reader io.Reader) SessionRecovery {
	return SessionRecovery{
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterSessionRecovery) Lower(value SessionRecovery) C.RustBuffer {
	return LowerIntoRustBuffer[SessionRecovery](c, value)
}

func (c FfiConverterSessionRecovery) LowerExternal(value SessionRecovery) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SessionRecovery](c, value))
}

func (c FfiConverterSessionRecovery) Write(writer io.Writer, value SessionRecovery) {
	FfiConverterBoolINSTANCE.Write(writer, value.Unlocked)
	FfiConverterBoolINSTANCE.Write(writer, value.Resumed)
}

type FfiDestroyerSessionRecovery struct{}

func (_ FfiDestroyerSessionRecovery) Destroy(value SessionRecovery) {
	value.Destroy()
}

type SpaceCreated struct {
	SpaceId             string
	SelfDeviceId        string
	IdentityFingerprint string
}

func (r *SpaceCreated) Destroy() {
	FfiDestroyerString{}.Destroy(r.SpaceId)
	FfiDestroyerString{}.Destroy(r.SelfDeviceId)
	FfiDestroyerString{}.Destroy(r.IdentityFingerprint)
}

type FfiConverterSpaceCreated struct{}

var FfiConverterSpaceCreatedINSTANCE = FfiConverterSpaceCreated{}

func (c FfiConverterSpaceCreated) Lift(rb RustBufferI) SpaceCreated {
	return LiftFromRustBuffer[SpaceCreated](c, rb)
}

func (c FfiConverterSpaceCreated) Read(reader io.Reader) SpaceCreated {
	return SpaceCreated{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterSpaceCreated) Lower(value SpaceCreated) C.RustBuffer {
	return LowerIntoRustBuffer[SpaceCreated](c, value)
}

func (c FfiConverterSpaceCreated) LowerExternal(value SpaceCreated) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SpaceCreated](c, value))
}

func (c FfiConverterSpaceCreated) Write(writer io.Writer, value SpaceCreated) {
	FfiConverterStringINSTANCE.Write(writer, value.SpaceId)
	FfiConverterStringINSTANCE.Write(writer, value.SelfDeviceId)
	FfiConverterStringINSTANCE.Write(writer, value.IdentityFingerprint)
}

type FfiDestroyerSpaceCreated struct{}

func (_ FfiDestroyerSpaceCreated) Destroy(value SpaceCreated) {
	value.Destroy()
}

type SpaceInvitation struct {
	InvitationCode string
	FullInvitation string
	ExpiresAtMs    int64
}

func (r *SpaceInvitation) Destroy() {
	FfiDestroyerString{}.Destroy(r.InvitationCode)
	FfiDestroyerString{}.Destroy(r.FullInvitation)
	FfiDestroyerInt64{}.Destroy(r.ExpiresAtMs)
}

type FfiConverterSpaceInvitation struct{}

var FfiConverterSpaceInvitationINSTANCE = FfiConverterSpaceInvitation{}

func (c FfiConverterSpaceInvitation) Lift(rb RustBufferI) SpaceInvitation {
	return LiftFromRustBuffer[SpaceInvitation](c, rb)
}

func (c FfiConverterSpaceInvitation) Read(reader io.Reader) SpaceInvitation {
	return SpaceInvitation{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterInt64INSTANCE.Read(reader),
	}
}

func (c FfiConverterSpaceInvitation) Lower(value SpaceInvitation) C.RustBuffer {
	return LowerIntoRustBuffer[SpaceInvitation](c, value)
}

func (c FfiConverterSpaceInvitation) LowerExternal(value SpaceInvitation) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SpaceInvitation](c, value))
}

func (c FfiConverterSpaceInvitation) Write(writer io.Writer, value SpaceInvitation) {
	FfiConverterStringINSTANCE.Write(writer, value.InvitationCode)
	FfiConverterStringINSTANCE.Write(writer, value.FullInvitation)
	FfiConverterInt64INSTANCE.Write(writer, value.ExpiresAtMs)
}

type FfiDestroyerSpaceInvitation struct{}

func (_ FfiDestroyerSpaceInvitation) Destroy(value SpaceInvitation) {
	value.Destroy()
}

type SpaceState struct {
	HasCompleted      bool
	RePairingRequired bool
	SpaceId           *string
	CurrentInvitation *SpaceInvitation
	DeviceName        *string
}

func (r *SpaceState) Destroy() {
	FfiDestroyerBool{}.Destroy(r.HasCompleted)
	FfiDestroyerBool{}.Destroy(r.RePairingRequired)
	FfiDestroyerOptionalString{}.Destroy(r.SpaceId)
	FfiDestroyerOptionalSpaceInvitation{}.Destroy(r.CurrentInvitation)
	FfiDestroyerOptionalString{}.Destroy(r.DeviceName)
}

type FfiConverterSpaceState struct{}

var FfiConverterSpaceStateINSTANCE = FfiConverterSpaceState{}

func (c FfiConverterSpaceState) Lift(rb RustBufferI) SpaceState {
	return LiftFromRustBuffer[SpaceState](c, rb)
}

func (c FfiConverterSpaceState) Read(reader io.Reader) SpaceState {
	return SpaceState{
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalSpaceInvitationINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterSpaceState) Lower(value SpaceState) C.RustBuffer {
	return LowerIntoRustBuffer[SpaceState](c, value)
}

func (c FfiConverterSpaceState) LowerExternal(value SpaceState) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SpaceState](c, value))
}

func (c FfiConverterSpaceState) Write(writer io.Writer, value SpaceState) {
	FfiConverterBoolINSTANCE.Write(writer, value.HasCompleted)
	FfiConverterBoolINSTANCE.Write(writer, value.RePairingRequired)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.SpaceId)
	FfiConverterOptionalSpaceInvitationINSTANCE.Write(writer, value.CurrentInvitation)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.DeviceName)
}

type FfiDestroyerSpaceState struct{}

func (_ FfiDestroyerSpaceState) Destroy(value SpaceState) {
	value.Destroy()
}

type WorkspaceConvergence struct {
	Phase                           WorkspaceConvergencePhase
	Revision                        uint64
	HistoryEventCount               uint64
	EffectiveMemberCount            uint64
	PendingRemovalDecisionDeviceIds []string
	PendingRemovalDecisionEventId   *string
	DivergedPeerDeviceIds           []string
	UpgradeRequiredPeerDeviceIds    []string
	ConvergenceDigest               *string
	Removed                         bool
	UpdatedAtMs                     int64
	FailureCategory                 *WorkspaceConvergenceFailureCategory
}

func (r *WorkspaceConvergence) Destroy() {
	FfiDestroyerWorkspaceConvergencePhase{}.Destroy(r.Phase)
	FfiDestroyerUint64{}.Destroy(r.Revision)
	FfiDestroyerUint64{}.Destroy(r.HistoryEventCount)
	FfiDestroyerUint64{}.Destroy(r.EffectiveMemberCount)
	FfiDestroyerSequenceString{}.Destroy(r.PendingRemovalDecisionDeviceIds)
	FfiDestroyerOptionalString{}.Destroy(r.PendingRemovalDecisionEventId)
	FfiDestroyerSequenceString{}.Destroy(r.DivergedPeerDeviceIds)
	FfiDestroyerSequenceString{}.Destroy(r.UpgradeRequiredPeerDeviceIds)
	FfiDestroyerOptionalString{}.Destroy(r.ConvergenceDigest)
	FfiDestroyerBool{}.Destroy(r.Removed)
	FfiDestroyerInt64{}.Destroy(r.UpdatedAtMs)
	FfiDestroyerOptionalWorkspaceConvergenceFailureCategory{}.Destroy(r.FailureCategory)
}

type FfiConverterWorkspaceConvergence struct{}

var FfiConverterWorkspaceConvergenceINSTANCE = FfiConverterWorkspaceConvergence{}

func (c FfiConverterWorkspaceConvergence) Lift(rb RustBufferI) WorkspaceConvergence {
	return LiftFromRustBuffer[WorkspaceConvergence](c, rb)
}

func (c FfiConverterWorkspaceConvergence) Read(reader io.Reader) WorkspaceConvergence {
	return WorkspaceConvergence{
		FfiConverterWorkspaceConvergencePhaseINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterSequenceStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterSequenceStringINSTANCE.Read(reader),
		FfiConverterSequenceStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterBoolINSTANCE.Read(reader),
		FfiConverterInt64INSTANCE.Read(reader),
		FfiConverterOptionalWorkspaceConvergenceFailureCategoryINSTANCE.Read(reader),
	}
}

func (c FfiConverterWorkspaceConvergence) Lower(value WorkspaceConvergence) C.RustBuffer {
	return LowerIntoRustBuffer[WorkspaceConvergence](c, value)
}

func (c FfiConverterWorkspaceConvergence) LowerExternal(value WorkspaceConvergence) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[WorkspaceConvergence](c, value))
}

func (c FfiConverterWorkspaceConvergence) Write(writer io.Writer, value WorkspaceConvergence) {
	FfiConverterWorkspaceConvergencePhaseINSTANCE.Write(writer, value.Phase)
	FfiConverterUint64INSTANCE.Write(writer, value.Revision)
	FfiConverterUint64INSTANCE.Write(writer, value.HistoryEventCount)
	FfiConverterUint64INSTANCE.Write(writer, value.EffectiveMemberCount)
	FfiConverterSequenceStringINSTANCE.Write(writer, value.PendingRemovalDecisionDeviceIds)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.PendingRemovalDecisionEventId)
	FfiConverterSequenceStringINSTANCE.Write(writer, value.DivergedPeerDeviceIds)
	FfiConverterSequenceStringINSTANCE.Write(writer, value.UpgradeRequiredPeerDeviceIds)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ConvergenceDigest)
	FfiConverterBoolINSTANCE.Write(writer, value.Removed)
	FfiConverterInt64INSTANCE.Write(writer, value.UpdatedAtMs)
	FfiConverterOptionalWorkspaceConvergenceFailureCategoryINSTANCE.Write(writer, value.FailureCategory)
}

type FfiDestroyerWorkspaceConvergence struct{}

func (_ FfiDestroyerWorkspaceConvergence) Destroy(value WorkspaceConvergence) {
	value.Destroy()
}

type BindingAnalyticsDeviceType uint

const (
	BindingAnalyticsDeviceTypeMobile  BindingAnalyticsDeviceType = 1
	BindingAnalyticsDeviceTypeDesktop BindingAnalyticsDeviceType = 2
)

type FfiConverterBindingAnalyticsDeviceType struct{}

var FfiConverterBindingAnalyticsDeviceTypeINSTANCE = FfiConverterBindingAnalyticsDeviceType{}

func (c FfiConverterBindingAnalyticsDeviceType) Lift(rb RustBufferI) BindingAnalyticsDeviceType {
	return LiftFromRustBuffer[BindingAnalyticsDeviceType](c, rb)
}

func (c FfiConverterBindingAnalyticsDeviceType) Lower(value BindingAnalyticsDeviceType) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsDeviceType](c, value)
}

func (c FfiConverterBindingAnalyticsDeviceType) LowerExternal(value BindingAnalyticsDeviceType) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsDeviceType](c, value))
}
func (FfiConverterBindingAnalyticsDeviceType) Read(reader io.Reader) BindingAnalyticsDeviceType {
	id := readInt32(reader)
	return BindingAnalyticsDeviceType(id)
}

func (FfiConverterBindingAnalyticsDeviceType) Write(writer io.Writer, value BindingAnalyticsDeviceType) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingAnalyticsDeviceType struct{}

func (_ FfiDestroyerBindingAnalyticsDeviceType) Destroy(value BindingAnalyticsDeviceType) {
}

type BindingAnalyticsHostError struct {
	err error
}

// Convenience method to turn *BindingAnalyticsHostError into error
// Avoiding treating nil pointer as non nil error interface
func (err *BindingAnalyticsHostError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err BindingAnalyticsHostError) Error() string {
	return fmt.Sprintf("BindingAnalyticsHostError: %s", err.err.Error())
}

func (err BindingAnalyticsHostError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrBindingAnalyticsHostErrorContextUnavailable = fmt.Errorf("BindingAnalyticsHostErrorContextUnavailable")
var ErrBindingAnalyticsHostErrorDeliveryFailed = fmt.Errorf("BindingAnalyticsHostErrorDeliveryFailed")
var ErrBindingAnalyticsHostErrorPersistenceFailed = fmt.Errorf("BindingAnalyticsHostErrorPersistenceFailed")
var ErrBindingAnalyticsHostErrorInvalidIdentity = fmt.Errorf("BindingAnalyticsHostErrorInvalidIdentity")

// Variant structs
type BindingAnalyticsHostErrorContextUnavailable struct {
}

func NewBindingAnalyticsHostErrorContextUnavailable() *BindingAnalyticsHostError {
	return &BindingAnalyticsHostError{err: &BindingAnalyticsHostErrorContextUnavailable{}}
}

func (e BindingAnalyticsHostErrorContextUnavailable) destroy() {
}

func (err BindingAnalyticsHostErrorContextUnavailable) Error() string {
	return fmt.Sprint("ContextUnavailable")
}

func (self BindingAnalyticsHostErrorContextUnavailable) Is(target error) bool {
	return target == ErrBindingAnalyticsHostErrorContextUnavailable
}

type BindingAnalyticsHostErrorDeliveryFailed struct {
}

func NewBindingAnalyticsHostErrorDeliveryFailed() *BindingAnalyticsHostError {
	return &BindingAnalyticsHostError{err: &BindingAnalyticsHostErrorDeliveryFailed{}}
}

func (e BindingAnalyticsHostErrorDeliveryFailed) destroy() {
}

func (err BindingAnalyticsHostErrorDeliveryFailed) Error() string {
	return fmt.Sprint("DeliveryFailed")
}

func (self BindingAnalyticsHostErrorDeliveryFailed) Is(target error) bool {
	return target == ErrBindingAnalyticsHostErrorDeliveryFailed
}

type BindingAnalyticsHostErrorPersistenceFailed struct {
}

func NewBindingAnalyticsHostErrorPersistenceFailed() *BindingAnalyticsHostError {
	return &BindingAnalyticsHostError{err: &BindingAnalyticsHostErrorPersistenceFailed{}}
}

func (e BindingAnalyticsHostErrorPersistenceFailed) destroy() {
}

func (err BindingAnalyticsHostErrorPersistenceFailed) Error() string {
	return fmt.Sprint("PersistenceFailed")
}

func (self BindingAnalyticsHostErrorPersistenceFailed) Is(target error) bool {
	return target == ErrBindingAnalyticsHostErrorPersistenceFailed
}

type BindingAnalyticsHostErrorInvalidIdentity struct {
}

func NewBindingAnalyticsHostErrorInvalidIdentity() *BindingAnalyticsHostError {
	return &BindingAnalyticsHostError{err: &BindingAnalyticsHostErrorInvalidIdentity{}}
}

func (e BindingAnalyticsHostErrorInvalidIdentity) destroy() {
}

func (err BindingAnalyticsHostErrorInvalidIdentity) Error() string {
	return fmt.Sprint("InvalidIdentity")
}

func (self BindingAnalyticsHostErrorInvalidIdentity) Is(target error) bool {
	return target == ErrBindingAnalyticsHostErrorInvalidIdentity
}

type FfiConverterBindingAnalyticsHostError struct{}

var FfiConverterBindingAnalyticsHostErrorINSTANCE = FfiConverterBindingAnalyticsHostError{}

func (c FfiConverterBindingAnalyticsHostError) Lift(eb RustBufferI) *BindingAnalyticsHostError {
	return LiftFromRustBuffer[*BindingAnalyticsHostError](c, eb)
}

func (c FfiConverterBindingAnalyticsHostError) Lower(value *BindingAnalyticsHostError) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingAnalyticsHostError](c, value)
}

func (c FfiConverterBindingAnalyticsHostError) LowerExternal(value *BindingAnalyticsHostError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingAnalyticsHostError](c, value))
}

func (c FfiConverterBindingAnalyticsHostError) Read(reader io.Reader) *BindingAnalyticsHostError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &BindingAnalyticsHostError{&BindingAnalyticsHostErrorContextUnavailable{}}
	case 2:
		return &BindingAnalyticsHostError{&BindingAnalyticsHostErrorDeliveryFailed{}}
	case 3:
		return &BindingAnalyticsHostError{&BindingAnalyticsHostErrorPersistenceFailed{}}
	case 4:
		return &BindingAnalyticsHostError{&BindingAnalyticsHostErrorInvalidIdentity{}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterBindingAnalyticsHostError.Read()", errorID))
	}
}

func (c FfiConverterBindingAnalyticsHostError) Write(writer io.Writer, value *BindingAnalyticsHostError) {
	switch variantValue := value.err.(type) {
	case *BindingAnalyticsHostErrorContextUnavailable:
		writeInt32(writer, 1)
	case *BindingAnalyticsHostErrorDeliveryFailed:
		writeInt32(writer, 2)
	case *BindingAnalyticsHostErrorPersistenceFailed:
		writeInt32(writer, 3)
	case *BindingAnalyticsHostErrorInvalidIdentity:
		writeInt32(writer, 4)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterBindingAnalyticsHostError.Write", value))
	}
}

type FfiDestroyerBindingAnalyticsHostError struct{}

func (_ FfiDestroyerBindingAnalyticsHostError) Destroy(value *BindingAnalyticsHostError) {
	switch variantValue := value.err.(type) {
	case BindingAnalyticsHostErrorContextUnavailable:
		variantValue.destroy()
	case BindingAnalyticsHostErrorDeliveryFailed:
		variantValue.destroy()
	case BindingAnalyticsHostErrorPersistenceFailed:
		variantValue.destroy()
	case BindingAnalyticsHostErrorInvalidIdentity:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerBindingAnalyticsHostError.Destroy", value))
	}
}

type BindingAnalyticsOs uint

const (
	BindingAnalyticsOsMacos   BindingAnalyticsOs = 1
	BindingAnalyticsOsWindows BindingAnalyticsOs = 2
	BindingAnalyticsOsLinux   BindingAnalyticsOs = 3
	BindingAnalyticsOsIos     BindingAnalyticsOs = 4
	BindingAnalyticsOsAndroid BindingAnalyticsOs = 5
	BindingAnalyticsOsOther   BindingAnalyticsOs = 6
)

type FfiConverterBindingAnalyticsOs struct{}

var FfiConverterBindingAnalyticsOsINSTANCE = FfiConverterBindingAnalyticsOs{}

func (c FfiConverterBindingAnalyticsOs) Lift(rb RustBufferI) BindingAnalyticsOs {
	return LiftFromRustBuffer[BindingAnalyticsOs](c, rb)
}

func (c FfiConverterBindingAnalyticsOs) Lower(value BindingAnalyticsOs) C.RustBuffer {
	return LowerIntoRustBuffer[BindingAnalyticsOs](c, value)
}

func (c FfiConverterBindingAnalyticsOs) LowerExternal(value BindingAnalyticsOs) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingAnalyticsOs](c, value))
}
func (FfiConverterBindingAnalyticsOs) Read(reader io.Reader) BindingAnalyticsOs {
	id := readInt32(reader)
	return BindingAnalyticsOs(id)
}

func (FfiConverterBindingAnalyticsOs) Write(writer io.Writer, value BindingAnalyticsOs) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingAnalyticsOs struct{}

func (_ FfiDestroyerBindingAnalyticsOs) Destroy(value BindingAnalyticsOs) {
}

type BindingCaptureEndReason uint

const (
	BindingCaptureEndReasonExpired                 BindingCaptureEndReason = 1
	BindingCaptureEndReasonRequested               BindingCaptureEndReason = 2
	BindingCaptureEndReasonSuspensionExpiryUnknown BindingCaptureEndReason = 3
	BindingCaptureEndReasonRuntimeShutdown         BindingCaptureEndReason = 4
)

type FfiConverterBindingCaptureEndReason struct{}

var FfiConverterBindingCaptureEndReasonINSTANCE = FfiConverterBindingCaptureEndReason{}

func (c FfiConverterBindingCaptureEndReason) Lift(rb RustBufferI) BindingCaptureEndReason {
	return LiftFromRustBuffer[BindingCaptureEndReason](c, rb)
}

func (c FfiConverterBindingCaptureEndReason) Lower(value BindingCaptureEndReason) C.RustBuffer {
	return LowerIntoRustBuffer[BindingCaptureEndReason](c, value)
}

func (c FfiConverterBindingCaptureEndReason) LowerExternal(value BindingCaptureEndReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingCaptureEndReason](c, value))
}
func (FfiConverterBindingCaptureEndReason) Read(reader io.Reader) BindingCaptureEndReason {
	id := readInt32(reader)
	return BindingCaptureEndReason(id)
}

func (FfiConverterBindingCaptureEndReason) Write(writer io.Writer, value BindingCaptureEndReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingCaptureEndReason struct{}

func (_ FfiDestroyerBindingCaptureEndReason) Destroy(value BindingCaptureEndReason) {
}

type BindingClipboardOrigin uint

const (
	BindingClipboardOriginLocal  BindingClipboardOrigin = 1
	BindingClipboardOriginRemote BindingClipboardOrigin = 2
)

type FfiConverterBindingClipboardOrigin struct{}

var FfiConverterBindingClipboardOriginINSTANCE = FfiConverterBindingClipboardOrigin{}

func (c FfiConverterBindingClipboardOrigin) Lift(rb RustBufferI) BindingClipboardOrigin {
	return LiftFromRustBuffer[BindingClipboardOrigin](c, rb)
}

func (c FfiConverterBindingClipboardOrigin) Lower(value BindingClipboardOrigin) C.RustBuffer {
	return LowerIntoRustBuffer[BindingClipboardOrigin](c, value)
}

func (c FfiConverterBindingClipboardOrigin) LowerExternal(value BindingClipboardOrigin) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingClipboardOrigin](c, value))
}
func (FfiConverterBindingClipboardOrigin) Read(reader io.Reader) BindingClipboardOrigin {
	id := readInt32(reader)
	return BindingClipboardOrigin(id)
}

func (FfiConverterBindingClipboardOrigin) Write(writer io.Writer, value BindingClipboardOrigin) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingClipboardOrigin struct{}

func (_ FfiDestroyerBindingClipboardOrigin) Destroy(value BindingClipboardOrigin) {
}

type BindingClipboardRepresentation interface {
	Destroy()
}
type BindingClipboardRepresentationInline struct {
	Format   string
	MimeType *string
	Bytes    []byte
}

func (e BindingClipboardRepresentationInline) Destroy() {
	FfiDestroyerString{}.Destroy(e.Format)
	FfiDestroyerOptionalString{}.Destroy(e.MimeType)
	FfiDestroyerBytes{}.Destroy(e.Bytes)
}

type BindingClipboardRepresentationFile struct {
	Format      string
	Handle      string
	DisplayName string
	MimeType    *string
	SizeBytes   uint64
}

func (e BindingClipboardRepresentationFile) Destroy() {
	FfiDestroyerString{}.Destroy(e.Format)
	FfiDestroyerString{}.Destroy(e.Handle)
	FfiDestroyerString{}.Destroy(e.DisplayName)
	FfiDestroyerOptionalString{}.Destroy(e.MimeType)
	FfiDestroyerUint64{}.Destroy(e.SizeBytes)
}

type FfiConverterBindingClipboardRepresentation struct{}

var FfiConverterBindingClipboardRepresentationINSTANCE = FfiConverterBindingClipboardRepresentation{}

func (c FfiConverterBindingClipboardRepresentation) Lift(rb RustBufferI) BindingClipboardRepresentation {
	return LiftFromRustBuffer[BindingClipboardRepresentation](c, rb)
}

func (c FfiConverterBindingClipboardRepresentation) Lower(value BindingClipboardRepresentation) C.RustBuffer {
	return LowerIntoRustBuffer[BindingClipboardRepresentation](c, value)
}

func (c FfiConverterBindingClipboardRepresentation) LowerExternal(value BindingClipboardRepresentation) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingClipboardRepresentation](c, value))
}
func (FfiConverterBindingClipboardRepresentation) Read(reader io.Reader) BindingClipboardRepresentation {
	id := readInt32(reader)
	switch id {
	case 1:
		return BindingClipboardRepresentationInline{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterBytesINSTANCE.Read(reader),
		}
	case 2:
		return BindingClipboardRepresentationFile{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
		}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterBindingClipboardRepresentation.Read()", id))
	}
}

func (FfiConverterBindingClipboardRepresentation) Write(writer io.Writer, value BindingClipboardRepresentation) {
	switch variant_value := value.(type) {
	case BindingClipboardRepresentationInline:
		writeInt32(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Format)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.MimeType)
		FfiConverterBytesINSTANCE.Write(writer, variant_value.Bytes)
	case BindingClipboardRepresentationFile:
		writeInt32(writer, 2)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Format)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Handle)
		FfiConverterStringINSTANCE.Write(writer, variant_value.DisplayName)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.MimeType)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.SizeBytes)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterBindingClipboardRepresentation.Write", value))
	}
}

type FfiDestroyerBindingClipboardRepresentation struct{}

func (_ FfiDestroyerBindingClipboardRepresentation) Destroy(value BindingClipboardRepresentation) {
	value.Destroy()
}

type BindingClipboardRestoreMode uint

const (
	BindingClipboardRestoreModeStandard  BindingClipboardRestoreMode = 1
	BindingClipboardRestoreModePlainText BindingClipboardRestoreMode = 2
	BindingClipboardRestoreModeFilePaths BindingClipboardRestoreMode = 3
)

type FfiConverterBindingClipboardRestoreMode struct{}

var FfiConverterBindingClipboardRestoreModeINSTANCE = FfiConverterBindingClipboardRestoreMode{}

func (c FfiConverterBindingClipboardRestoreMode) Lift(rb RustBufferI) BindingClipboardRestoreMode {
	return LiftFromRustBuffer[BindingClipboardRestoreMode](c, rb)
}

func (c FfiConverterBindingClipboardRestoreMode) Lower(value BindingClipboardRestoreMode) C.RustBuffer {
	return LowerIntoRustBuffer[BindingClipboardRestoreMode](c, value)
}

func (c FfiConverterBindingClipboardRestoreMode) LowerExternal(value BindingClipboardRestoreMode) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingClipboardRestoreMode](c, value))
}
func (FfiConverterBindingClipboardRestoreMode) Read(reader io.Reader) BindingClipboardRestoreMode {
	id := readInt32(reader)
	return BindingClipboardRestoreMode(id)
}

func (FfiConverterBindingClipboardRestoreMode) Write(writer io.Writer, value BindingClipboardRestoreMode) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingClipboardRestoreMode struct{}

func (_ FfiDestroyerBindingClipboardRestoreMode) Destroy(value BindingClipboardRestoreMode) {
}

type BindingClipboardRestoreOutcome uint

const (
	BindingClipboardRestoreOutcomeRestored           BindingClipboardRestoreOutcome = 1
	BindingClipboardRestoreOutcomePayloadUnavailable BindingClipboardRestoreOutcome = 2
	BindingClipboardRestoreOutcomeNotApplicable      BindingClipboardRestoreOutcome = 3
)

type FfiConverterBindingClipboardRestoreOutcome struct{}

var FfiConverterBindingClipboardRestoreOutcomeINSTANCE = FfiConverterBindingClipboardRestoreOutcome{}

func (c FfiConverterBindingClipboardRestoreOutcome) Lift(rb RustBufferI) BindingClipboardRestoreOutcome {
	return LiftFromRustBuffer[BindingClipboardRestoreOutcome](c, rb)
}

func (c FfiConverterBindingClipboardRestoreOutcome) Lower(value BindingClipboardRestoreOutcome) C.RustBuffer {
	return LowerIntoRustBuffer[BindingClipboardRestoreOutcome](c, value)
}

func (c FfiConverterBindingClipboardRestoreOutcome) LowerExternal(value BindingClipboardRestoreOutcome) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingClipboardRestoreOutcome](c, value))
}
func (FfiConverterBindingClipboardRestoreOutcome) Read(reader io.Reader) BindingClipboardRestoreOutcome {
	id := readInt32(reader)
	return BindingClipboardRestoreOutcome(id)
}

func (FfiConverterBindingClipboardRestoreOutcome) Write(writer io.Writer, value BindingClipboardRestoreOutcome) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingClipboardRestoreOutcome struct{}

func (_ FfiDestroyerBindingClipboardRestoreOutcome) Destroy(value BindingClipboardRestoreOutcome) {
}

type BindingDeploymentEnvironment uint

const (
	BindingDeploymentEnvironmentDevelopment BindingDeploymentEnvironment = 1
	BindingDeploymentEnvironmentTest        BindingDeploymentEnvironment = 2
	BindingDeploymentEnvironmentStaging     BindingDeploymentEnvironment = 3
	BindingDeploymentEnvironmentProduction  BindingDeploymentEnvironment = 4
)

type FfiConverterBindingDeploymentEnvironment struct{}

var FfiConverterBindingDeploymentEnvironmentINSTANCE = FfiConverterBindingDeploymentEnvironment{}

func (c FfiConverterBindingDeploymentEnvironment) Lift(rb RustBufferI) BindingDeploymentEnvironment {
	return LiftFromRustBuffer[BindingDeploymentEnvironment](c, rb)
}

func (c FfiConverterBindingDeploymentEnvironment) Lower(value BindingDeploymentEnvironment) C.RustBuffer {
	return LowerIntoRustBuffer[BindingDeploymentEnvironment](c, value)
}

func (c FfiConverterBindingDeploymentEnvironment) LowerExternal(value BindingDeploymentEnvironment) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingDeploymentEnvironment](c, value))
}
func (FfiConverterBindingDeploymentEnvironment) Read(reader io.Reader) BindingDeploymentEnvironment {
	id := readInt32(reader)
	return BindingDeploymentEnvironment(id)
}

func (FfiConverterBindingDeploymentEnvironment) Write(writer io.Writer, value BindingDeploymentEnvironment) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingDeploymentEnvironment struct{}

func (_ FfiDestroyerBindingDeploymentEnvironment) Destroy(value BindingDeploymentEnvironment) {
}

type BindingEngineState uint

const (
	BindingEngineStateRunning      BindingEngineState = 1
	BindingEngineStateQuiescing    BindingEngineState = 2
	BindingEngineStateQuiesced     BindingEngineState = 3
	BindingEngineStateSuspended    BindingEngineState = 4
	BindingEngineStateShuttingDown BindingEngineState = 5
	BindingEngineStateStopped      BindingEngineState = 6
)

type FfiConverterBindingEngineState struct{}

var FfiConverterBindingEngineStateINSTANCE = FfiConverterBindingEngineState{}

func (c FfiConverterBindingEngineState) Lift(rb RustBufferI) BindingEngineState {
	return LiftFromRustBuffer[BindingEngineState](c, rb)
}

func (c FfiConverterBindingEngineState) Lower(value BindingEngineState) C.RustBuffer {
	return LowerIntoRustBuffer[BindingEngineState](c, value)
}

func (c FfiConverterBindingEngineState) LowerExternal(value BindingEngineState) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingEngineState](c, value))
}
func (FfiConverterBindingEngineState) Read(reader io.Reader) BindingEngineState {
	id := readInt32(reader)
	return BindingEngineState(id)
}

func (FfiConverterBindingEngineState) Write(writer io.Writer, value BindingEngineState) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingEngineState struct{}

func (_ FfiDestroyerBindingEngineState) Destroy(value BindingEngineState) {
}

type BindingError struct {
	err error
}

// Convenience method to turn *BindingError into error
// Avoiding treating nil pointer as non nil error interface
func (err *BindingError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err BindingError) Error() string {
	return fmt.Sprintf("BindingError: %s", err.err.Error())
}

func (err BindingError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrBindingErrorEngine = fmt.Errorf("BindingErrorEngine")
var ErrBindingErrorHostUnavailable = fmt.Errorf("BindingErrorHostUnavailable")
var ErrBindingErrorHostPermissionDenied = fmt.Errorf("BindingErrorHostPermissionDenied")
var ErrBindingErrorHostInvalidHandle = fmt.Errorf("BindingErrorHostInvalidHandle")
var ErrBindingErrorHostIo = fmt.Errorf("BindingErrorHostIo")
var ErrBindingErrorRuntimeUnavailable = fmt.Errorf("BindingErrorRuntimeUnavailable")
var ErrBindingErrorAlreadyStopped = fmt.Errorf("BindingErrorAlreadyStopped")
var ErrBindingErrorObservabilityConfigInvalid = fmt.Errorf("BindingErrorObservabilityConfigInvalid")
var ErrBindingErrorObservabilityConfigConflict = fmt.Errorf("BindingErrorObservabilityConfigConflict")
var ErrBindingErrorObservabilityRuntimeUnavailable = fmt.Errorf("BindingErrorObservabilityRuntimeUnavailable")
var ErrBindingErrorObservabilityNotInstalled = fmt.Errorf("BindingErrorObservabilityNotInstalled")
var ErrBindingErrorUnexpectedResult = fmt.Errorf("BindingErrorUnexpectedResult")

// Variant structs
type BindingErrorEngine struct {
	Code      uint32
	Category  BindingErrorCategory
	Retryable bool
}

func NewBindingErrorEngine(
	code uint32,
	category BindingErrorCategory,
	retryable bool,
) *BindingError {
	return &BindingError{err: &BindingErrorEngine{
		Code:      code,
		Category:  category,
		Retryable: retryable}}
}

func (e BindingErrorEngine) destroy() {
	FfiDestroyerUint32{}.Destroy(e.Code)
	FfiDestroyerBindingErrorCategory{}.Destroy(e.Category)
	FfiDestroyerBool{}.Destroy(e.Retryable)
}

func (err BindingErrorEngine) Error() string {
	return fmt.Sprint("Engine",
		": ",

		"Code=",
		err.Code,
		", ",
		"Category=",
		err.Category,
		", ",
		"Retryable=",
		err.Retryable,
	)
}

func (self BindingErrorEngine) Is(target error) bool {
	return target == ErrBindingErrorEngine
}

type BindingErrorHostUnavailable struct {
}

func NewBindingErrorHostUnavailable() *BindingError {
	return &BindingError{err: &BindingErrorHostUnavailable{}}
}

func (e BindingErrorHostUnavailable) destroy() {
}

func (err BindingErrorHostUnavailable) Error() string {
	return fmt.Sprint("HostUnavailable")
}

func (self BindingErrorHostUnavailable) Is(target error) bool {
	return target == ErrBindingErrorHostUnavailable
}

type BindingErrorHostPermissionDenied struct {
}

func NewBindingErrorHostPermissionDenied() *BindingError {
	return &BindingError{err: &BindingErrorHostPermissionDenied{}}
}

func (e BindingErrorHostPermissionDenied) destroy() {
}

func (err BindingErrorHostPermissionDenied) Error() string {
	return fmt.Sprint("HostPermissionDenied")
}

func (self BindingErrorHostPermissionDenied) Is(target error) bool {
	return target == ErrBindingErrorHostPermissionDenied
}

type BindingErrorHostInvalidHandle struct {
}

func NewBindingErrorHostInvalidHandle() *BindingError {
	return &BindingError{err: &BindingErrorHostInvalidHandle{}}
}

func (e BindingErrorHostInvalidHandle) destroy() {
}

func (err BindingErrorHostInvalidHandle) Error() string {
	return fmt.Sprint("HostInvalidHandle")
}

func (self BindingErrorHostInvalidHandle) Is(target error) bool {
	return target == ErrBindingErrorHostInvalidHandle
}

type BindingErrorHostIo struct {
}

func NewBindingErrorHostIo() *BindingError {
	return &BindingError{err: &BindingErrorHostIo{}}
}

func (e BindingErrorHostIo) destroy() {
}

func (err BindingErrorHostIo) Error() string {
	return fmt.Sprint("HostIo")
}

func (self BindingErrorHostIo) Is(target error) bool {
	return target == ErrBindingErrorHostIo
}

type BindingErrorRuntimeUnavailable struct {
}

func NewBindingErrorRuntimeUnavailable() *BindingError {
	return &BindingError{err: &BindingErrorRuntimeUnavailable{}}
}

func (e BindingErrorRuntimeUnavailable) destroy() {
}

func (err BindingErrorRuntimeUnavailable) Error() string {
	return fmt.Sprint("RuntimeUnavailable")
}

func (self BindingErrorRuntimeUnavailable) Is(target error) bool {
	return target == ErrBindingErrorRuntimeUnavailable
}

type BindingErrorAlreadyStopped struct {
}

func NewBindingErrorAlreadyStopped() *BindingError {
	return &BindingError{err: &BindingErrorAlreadyStopped{}}
}

func (e BindingErrorAlreadyStopped) destroy() {
}

func (err BindingErrorAlreadyStopped) Error() string {
	return fmt.Sprint("AlreadyStopped")
}

func (self BindingErrorAlreadyStopped) Is(target error) bool {
	return target == ErrBindingErrorAlreadyStopped
}

type BindingErrorObservabilityConfigInvalid struct {
}

func NewBindingErrorObservabilityConfigInvalid() *BindingError {
	return &BindingError{err: &BindingErrorObservabilityConfigInvalid{}}
}

func (e BindingErrorObservabilityConfigInvalid) destroy() {
}

func (err BindingErrorObservabilityConfigInvalid) Error() string {
	return fmt.Sprint("ObservabilityConfigInvalid")
}

func (self BindingErrorObservabilityConfigInvalid) Is(target error) bool {
	return target == ErrBindingErrorObservabilityConfigInvalid
}

type BindingErrorObservabilityConfigConflict struct {
}

func NewBindingErrorObservabilityConfigConflict() *BindingError {
	return &BindingError{err: &BindingErrorObservabilityConfigConflict{}}
}

func (e BindingErrorObservabilityConfigConflict) destroy() {
}

func (err BindingErrorObservabilityConfigConflict) Error() string {
	return fmt.Sprint("ObservabilityConfigConflict")
}

func (self BindingErrorObservabilityConfigConflict) Is(target error) bool {
	return target == ErrBindingErrorObservabilityConfigConflict
}

type BindingErrorObservabilityRuntimeUnavailable struct {
}

func NewBindingErrorObservabilityRuntimeUnavailable() *BindingError {
	return &BindingError{err: &BindingErrorObservabilityRuntimeUnavailable{}}
}

func (e BindingErrorObservabilityRuntimeUnavailable) destroy() {
}

func (err BindingErrorObservabilityRuntimeUnavailable) Error() string {
	return fmt.Sprint("ObservabilityRuntimeUnavailable")
}

func (self BindingErrorObservabilityRuntimeUnavailable) Is(target error) bool {
	return target == ErrBindingErrorObservabilityRuntimeUnavailable
}

type BindingErrorObservabilityNotInstalled struct {
}

func NewBindingErrorObservabilityNotInstalled() *BindingError {
	return &BindingError{err: &BindingErrorObservabilityNotInstalled{}}
}

func (e BindingErrorObservabilityNotInstalled) destroy() {
}

func (err BindingErrorObservabilityNotInstalled) Error() string {
	return fmt.Sprint("ObservabilityNotInstalled")
}

func (self BindingErrorObservabilityNotInstalled) Is(target error) bool {
	return target == ErrBindingErrorObservabilityNotInstalled
}

type BindingErrorUnexpectedResult struct {
}

func NewBindingErrorUnexpectedResult() *BindingError {
	return &BindingError{err: &BindingErrorUnexpectedResult{}}
}

func (e BindingErrorUnexpectedResult) destroy() {
}

func (err BindingErrorUnexpectedResult) Error() string {
	return fmt.Sprint("UnexpectedResult")
}

func (self BindingErrorUnexpectedResult) Is(target error) bool {
	return target == ErrBindingErrorUnexpectedResult
}

type FfiConverterBindingError struct{}

var FfiConverterBindingErrorINSTANCE = FfiConverterBindingError{}

func (c FfiConverterBindingError) Lift(eb RustBufferI) *BindingError {
	return LiftFromRustBuffer[*BindingError](c, eb)
}

func (c FfiConverterBindingError) Lower(value *BindingError) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingError](c, value)
}

func (c FfiConverterBindingError) LowerExternal(value *BindingError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingError](c, value))
}

func (c FfiConverterBindingError) Read(reader io.Reader) *BindingError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &BindingError{&BindingErrorEngine{
			Code:      FfiConverterUint32INSTANCE.Read(reader),
			Category:  FfiConverterBindingErrorCategoryINSTANCE.Read(reader),
			Retryable: FfiConverterBoolINSTANCE.Read(reader),
		}}
	case 2:
		return &BindingError{&BindingErrorHostUnavailable{}}
	case 3:
		return &BindingError{&BindingErrorHostPermissionDenied{}}
	case 4:
		return &BindingError{&BindingErrorHostInvalidHandle{}}
	case 5:
		return &BindingError{&BindingErrorHostIo{}}
	case 6:
		return &BindingError{&BindingErrorRuntimeUnavailable{}}
	case 7:
		return &BindingError{&BindingErrorAlreadyStopped{}}
	case 8:
		return &BindingError{&BindingErrorObservabilityConfigInvalid{}}
	case 9:
		return &BindingError{&BindingErrorObservabilityConfigConflict{}}
	case 10:
		return &BindingError{&BindingErrorObservabilityRuntimeUnavailable{}}
	case 11:
		return &BindingError{&BindingErrorObservabilityNotInstalled{}}
	case 12:
		return &BindingError{&BindingErrorUnexpectedResult{}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterBindingError.Read()", errorID))
	}
}

func (c FfiConverterBindingError) Write(writer io.Writer, value *BindingError) {
	switch variantValue := value.err.(type) {
	case *BindingErrorEngine:
		writeInt32(writer, 1)
		FfiConverterUint32INSTANCE.Write(writer, variantValue.Code)
		FfiConverterBindingErrorCategoryINSTANCE.Write(writer, variantValue.Category)
		FfiConverterBoolINSTANCE.Write(writer, variantValue.Retryable)
	case *BindingErrorHostUnavailable:
		writeInt32(writer, 2)
	case *BindingErrorHostPermissionDenied:
		writeInt32(writer, 3)
	case *BindingErrorHostInvalidHandle:
		writeInt32(writer, 4)
	case *BindingErrorHostIo:
		writeInt32(writer, 5)
	case *BindingErrorRuntimeUnavailable:
		writeInt32(writer, 6)
	case *BindingErrorAlreadyStopped:
		writeInt32(writer, 7)
	case *BindingErrorObservabilityConfigInvalid:
		writeInt32(writer, 8)
	case *BindingErrorObservabilityConfigConflict:
		writeInt32(writer, 9)
	case *BindingErrorObservabilityRuntimeUnavailable:
		writeInt32(writer, 10)
	case *BindingErrorObservabilityNotInstalled:
		writeInt32(writer, 11)
	case *BindingErrorUnexpectedResult:
		writeInt32(writer, 12)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterBindingError.Write", value))
	}
}

type FfiDestroyerBindingError struct{}

func (_ FfiDestroyerBindingError) Destroy(value *BindingError) {
	switch variantValue := value.err.(type) {
	case BindingErrorEngine:
		variantValue.destroy()
	case BindingErrorHostUnavailable:
		variantValue.destroy()
	case BindingErrorHostPermissionDenied:
		variantValue.destroy()
	case BindingErrorHostInvalidHandle:
		variantValue.destroy()
	case BindingErrorHostIo:
		variantValue.destroy()
	case BindingErrorRuntimeUnavailable:
		variantValue.destroy()
	case BindingErrorAlreadyStopped:
		variantValue.destroy()
	case BindingErrorObservabilityConfigInvalid:
		variantValue.destroy()
	case BindingErrorObservabilityConfigConflict:
		variantValue.destroy()
	case BindingErrorObservabilityRuntimeUnavailable:
		variantValue.destroy()
	case BindingErrorObservabilityNotInstalled:
		variantValue.destroy()
	case BindingErrorUnexpectedResult:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerBindingError.Destroy", value))
	}
}

type BindingErrorCategory uint

const (
	BindingErrorCategoryInvalidInput     BindingErrorCategory = 1
	BindingErrorCategoryInvalidState     BindingErrorCategory = 2
	BindingErrorCategoryUnauthorized     BindingErrorCategory = 3
	BindingErrorCategoryNotFound         BindingErrorCategory = 4
	BindingErrorCategoryConflict         BindingErrorCategory = 5
	BindingErrorCategoryUnavailable      BindingErrorCategory = 6
	BindingErrorCategoryDeadlineExceeded BindingErrorCategory = 7
	BindingErrorCategoryInternal         BindingErrorCategory = 8
)

type FfiConverterBindingErrorCategory struct{}

var FfiConverterBindingErrorCategoryINSTANCE = FfiConverterBindingErrorCategory{}

func (c FfiConverterBindingErrorCategory) Lift(rb RustBufferI) BindingErrorCategory {
	return LiftFromRustBuffer[BindingErrorCategory](c, rb)
}

func (c FfiConverterBindingErrorCategory) Lower(value BindingErrorCategory) C.RustBuffer {
	return LowerIntoRustBuffer[BindingErrorCategory](c, value)
}

func (c FfiConverterBindingErrorCategory) LowerExternal(value BindingErrorCategory) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingErrorCategory](c, value))
}
func (FfiConverterBindingErrorCategory) Read(reader io.Reader) BindingErrorCategory {
	id := readInt32(reader)
	return BindingErrorCategory(id)
}

func (FfiConverterBindingErrorCategory) Write(writer io.Writer, value BindingErrorCategory) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingErrorCategory struct{}

func (_ FfiDestroyerBindingErrorCategory) Destroy(value BindingErrorCategory) {
}

type BindingEvent interface {
	Destroy()
}
type BindingEventStateChanged struct {
	State BindingEngineState
}

func (e BindingEventStateChanged) Destroy() {
	FfiDestroyerBindingEngineState{}.Destroy(e.State)
}

type BindingEventOperationFinished struct {
	OperationId string
	Terminal    BindingOperationTerminal
	Failure     *BindingFailure
}

func (e BindingEventOperationFinished) Destroy() {
	FfiDestroyerString{}.Destroy(e.OperationId)
	FfiDestroyerBindingOperationTerminal{}.Destroy(e.Terminal)
	FfiDestroyerOptionalBindingFailure{}.Destroy(e.Failure)
}

type BindingEventLifecycleFailed struct {
	Action  BindingLifecycleAction
	Failure BindingFailure
}

func (e BindingEventLifecycleFailed) Destroy() {
	FfiDestroyerBindingLifecycleAction{}.Destroy(e.Action)
	FfiDestroyerBindingFailure{}.Destroy(e.Failure)
}

type BindingEventRefreshRequired struct {
	Reason BindingRefreshReason
}

func (e BindingEventRefreshRequired) Destroy() {
	FfiDestroyerBindingRefreshReason{}.Destroy(e.Reason)
}

type BindingEventFatal struct {
	Failure BindingFailure
}

func (e BindingEventFatal) Destroy() {
	FfiDestroyerBindingFailure{}.Destroy(e.Failure)
}

type BindingEventIncomingEntry struct {
	EntryId   string
	AttemptId *string
	Preview   string
	Origin    BindingClipboardOrigin
}

func (e BindingEventIncomingEntry) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerOptionalString{}.Destroy(e.AttemptId)
	FfiDestroyerString{}.Destroy(e.Preview)
	FfiDestroyerBindingClipboardOrigin{}.Destroy(e.Origin)
}

type BindingEventIncomingPending struct {
	EntryId    string
	AttemptId  *string
	FromDevice string
	TotalBytes *uint64
	Filenames  []string
}

func (e BindingEventIncomingPending) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerOptionalString{}.Destroy(e.AttemptId)
	FfiDestroyerString{}.Destroy(e.FromDevice)
	FfiDestroyerOptionalUint64{}.Destroy(e.TotalBytes)
	FfiDestroyerSequenceString{}.Destroy(e.Filenames)
}

type BindingEventReceiveAttemptStateChanged struct {
	EntryId   string
	AttemptId string
	State     string
}

func (e BindingEventReceiveAttemptStateChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerString{}.Destroy(e.AttemptId)
	FfiDestroyerString{}.Destroy(e.State)
}

type BindingEventDeliveryStatusChanged struct {
	EntryId        string
	TargetDeviceId string
}

func (e BindingEventDeliveryStatusChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerString{}.Destroy(e.TargetDeviceId)
}

type BindingEventPeerPresenceChanged struct {
	DeviceId string
	State    string
	AtMs     int64
}

func (e BindingEventPeerPresenceChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.DeviceId)
	FfiDestroyerString{}.Destroy(e.State)
	FfiDestroyerInt64{}.Destroy(e.AtMs)
}

type BindingEventDeviceTrustChanged struct {
	Revision uint64
}

func (e BindingEventDeviceTrustChanged) Destroy() {
	FfiDestroyerUint64{}.Destroy(e.Revision)
}

type BindingEventTransferProgress struct {
	TransferId     string
	EntryId        *string
	AttemptId      *string
	PeerId         string
	Direction      BindingTransferDirection
	CompletedBytes uint64
	TotalBytes     *uint64
}

func (e BindingEventTransferProgress) Destroy() {
	FfiDestroyerString{}.Destroy(e.TransferId)
	FfiDestroyerOptionalString{}.Destroy(e.EntryId)
	FfiDestroyerOptionalString{}.Destroy(e.AttemptId)
	FfiDestroyerString{}.Destroy(e.PeerId)
	FfiDestroyerBindingTransferDirection{}.Destroy(e.Direction)
	FfiDestroyerUint64{}.Destroy(e.CompletedBytes)
	FfiDestroyerOptionalUint64{}.Destroy(e.TotalBytes)
}

type BindingEventTransferStatusChanged struct {
	TransferId string
	EntryId    *string
	AttemptId  *string
	Status     string
	Reason     *string
}

func (e BindingEventTransferStatusChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.TransferId)
	FfiDestroyerOptionalString{}.Destroy(e.EntryId)
	FfiDestroyerOptionalString{}.Destroy(e.AttemptId)
	FfiDestroyerString{}.Destroy(e.Status)
	FfiDestroyerOptionalString{}.Destroy(e.Reason)
}

type BindingEventActiveClipboardChanged struct {
	SnapshotHash  string
	EntryId       string
	ActivatedAtMs int64
	ActivatedBy   string
}

func (e BindingEventActiveClipboardChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.SnapshotHash)
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerInt64{}.Destroy(e.ActivatedAtMs)
	FfiDestroyerString{}.Destroy(e.ActivatedBy)
}

type BindingEventNetworkRecoveryChanged struct {
	Phase         string
	Retryable     bool
	NextRetryInMs *uint64
}

func (e BindingEventNetworkRecoveryChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.Phase)
	FfiDestroyerBool{}.Destroy(e.Retryable)
	FfiDestroyerOptionalUint64{}.Destroy(e.NextRetryInMs)
}

type BindingEventRePairingRequired struct {
	Scope BindingRePairingScope
}

func (e BindingEventRePairingRequired) Destroy() {
	FfiDestroyerBindingRePairingScope{}.Destroy(e.Scope)
}

type BindingEventChanged struct {
	Kind string
}

func (e BindingEventChanged) Destroy() {
	FfiDestroyerString{}.Destroy(e.Kind)
}

type FfiConverterBindingEvent struct{}

var FfiConverterBindingEventINSTANCE = FfiConverterBindingEvent{}

func (c FfiConverterBindingEvent) Lift(rb RustBufferI) BindingEvent {
	return LiftFromRustBuffer[BindingEvent](c, rb)
}

func (c FfiConverterBindingEvent) Lower(value BindingEvent) C.RustBuffer {
	return LowerIntoRustBuffer[BindingEvent](c, value)
}

func (c FfiConverterBindingEvent) LowerExternal(value BindingEvent) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingEvent](c, value))
}
func (FfiConverterBindingEvent) Read(reader io.Reader) BindingEvent {
	id := readInt32(reader)
	switch id {
	case 1:
		return BindingEventStateChanged{
			FfiConverterBindingEngineStateINSTANCE.Read(reader),
		}
	case 2:
		return BindingEventOperationFinished{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBindingOperationTerminalINSTANCE.Read(reader),
			FfiConverterOptionalBindingFailureINSTANCE.Read(reader),
		}
	case 3:
		return BindingEventLifecycleFailed{
			FfiConverterBindingLifecycleActionINSTANCE.Read(reader),
			FfiConverterBindingFailureINSTANCE.Read(reader),
		}
	case 4:
		return BindingEventRefreshRequired{
			FfiConverterBindingRefreshReasonINSTANCE.Read(reader),
		}
	case 5:
		return BindingEventFatal{
			FfiConverterBindingFailureINSTANCE.Read(reader),
		}
	case 6:
		return BindingEventIncomingEntry{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBindingClipboardOriginINSTANCE.Read(reader),
		}
	case 7:
		return BindingEventIncomingPending{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalUint64INSTANCE.Read(reader),
			FfiConverterSequenceStringINSTANCE.Read(reader),
		}
	case 8:
		return BindingEventReceiveAttemptStateChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
		}
	case 9:
		return BindingEventDeliveryStatusChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
		}
	case 10:
		return BindingEventPeerPresenceChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterInt64INSTANCE.Read(reader),
		}
	case 11:
		return BindingEventDeviceTrustChanged{
			FfiConverterUint64INSTANCE.Read(reader),
		}
	case 12:
		return BindingEventTransferProgress{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBindingTransferDirectionINSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
			FfiConverterOptionalUint64INSTANCE.Read(reader),
		}
	case 13:
		return BindingEventTransferStatusChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
		}
	case 14:
		return BindingEventActiveClipboardChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterInt64INSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
		}
	case 15:
		return BindingEventNetworkRecoveryChanged{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
			FfiConverterOptionalUint64INSTANCE.Read(reader),
		}
	case 16:
		return BindingEventRePairingRequired{
			FfiConverterBindingRePairingScopeINSTANCE.Read(reader),
		}
	case 17:
		return BindingEventChanged{
			FfiConverterStringINSTANCE.Read(reader),
		}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterBindingEvent.Read()", id))
	}
}

func (FfiConverterBindingEvent) Write(writer io.Writer, value BindingEvent) {
	switch variant_value := value.(type) {
	case BindingEventStateChanged:
		writeInt32(writer, 1)
		FfiConverterBindingEngineStateINSTANCE.Write(writer, variant_value.State)
	case BindingEventOperationFinished:
		writeInt32(writer, 2)
		FfiConverterStringINSTANCE.Write(writer, variant_value.OperationId)
		FfiConverterBindingOperationTerminalINSTANCE.Write(writer, variant_value.Terminal)
		FfiConverterOptionalBindingFailureINSTANCE.Write(writer, variant_value.Failure)
	case BindingEventLifecycleFailed:
		writeInt32(writer, 3)
		FfiConverterBindingLifecycleActionINSTANCE.Write(writer, variant_value.Action)
		FfiConverterBindingFailureINSTANCE.Write(writer, variant_value.Failure)
	case BindingEventRefreshRequired:
		writeInt32(writer, 4)
		FfiConverterBindingRefreshReasonINSTANCE.Write(writer, variant_value.Reason)
	case BindingEventFatal:
		writeInt32(writer, 5)
		FfiConverterBindingFailureINSTANCE.Write(writer, variant_value.Failure)
	case BindingEventIncomingEntry:
		writeInt32(writer, 6)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.AttemptId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Preview)
		FfiConverterBindingClipboardOriginINSTANCE.Write(writer, variant_value.Origin)
	case BindingEventIncomingPending:
		writeInt32(writer, 7)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.AttemptId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.FromDevice)
		FfiConverterOptionalUint64INSTANCE.Write(writer, variant_value.TotalBytes)
		FfiConverterSequenceStringINSTANCE.Write(writer, variant_value.Filenames)
	case BindingEventReceiveAttemptStateChanged:
		writeInt32(writer, 8)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.AttemptId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.State)
	case BindingEventDeliveryStatusChanged:
		writeInt32(writer, 9)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.TargetDeviceId)
	case BindingEventPeerPresenceChanged:
		writeInt32(writer, 10)
		FfiConverterStringINSTANCE.Write(writer, variant_value.DeviceId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.State)
		FfiConverterInt64INSTANCE.Write(writer, variant_value.AtMs)
	case BindingEventDeviceTrustChanged:
		writeInt32(writer, 11)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Revision)
	case BindingEventTransferProgress:
		writeInt32(writer, 12)
		FfiConverterStringINSTANCE.Write(writer, variant_value.TransferId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.AttemptId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.PeerId)
		FfiConverterBindingTransferDirectionINSTANCE.Write(writer, variant_value.Direction)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.CompletedBytes)
		FfiConverterOptionalUint64INSTANCE.Write(writer, variant_value.TotalBytes)
	case BindingEventTransferStatusChanged:
		writeInt32(writer, 13)
		FfiConverterStringINSTANCE.Write(writer, variant_value.TransferId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.AttemptId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Status)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.Reason)
	case BindingEventActiveClipboardChanged:
		writeInt32(writer, 14)
		FfiConverterStringINSTANCE.Write(writer, variant_value.SnapshotHash)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterInt64INSTANCE.Write(writer, variant_value.ActivatedAtMs)
		FfiConverterStringINSTANCE.Write(writer, variant_value.ActivatedBy)
	case BindingEventNetworkRecoveryChanged:
		writeInt32(writer, 15)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Phase)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.Retryable)
		FfiConverterOptionalUint64INSTANCE.Write(writer, variant_value.NextRetryInMs)
	case BindingEventRePairingRequired:
		writeInt32(writer, 16)
		FfiConverterBindingRePairingScopeINSTANCE.Write(writer, variant_value.Scope)
	case BindingEventChanged:
		writeInt32(writer, 17)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Kind)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterBindingEvent.Write", value))
	}
}

type FfiDestroyerBindingEvent struct{}

func (_ FfiDestroyerBindingEvent) Destroy(value BindingEvent) {
	value.Destroy()
}

type BindingHostDiagnosticAction uint

const (
	BindingHostDiagnosticActionRuntimeStart     BindingHostDiagnosticAction = 1
	BindingHostDiagnosticActionRuntimeStop      BindingHostDiagnosticAction = 2
	BindingHostDiagnosticActionOwnershipAcquire BindingHostDiagnosticAction = 3
	BindingHostDiagnosticActionSecurityPrepare  BindingHostDiagnosticAction = 4
)

type FfiConverterBindingHostDiagnosticAction struct{}

var FfiConverterBindingHostDiagnosticActionINSTANCE = FfiConverterBindingHostDiagnosticAction{}

func (c FfiConverterBindingHostDiagnosticAction) Lift(rb RustBufferI) BindingHostDiagnosticAction {
	return LiftFromRustBuffer[BindingHostDiagnosticAction](c, rb)
}

func (c FfiConverterBindingHostDiagnosticAction) Lower(value BindingHostDiagnosticAction) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticAction](c, value)
}

func (c FfiConverterBindingHostDiagnosticAction) LowerExternal(value BindingHostDiagnosticAction) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticAction](c, value))
}
func (FfiConverterBindingHostDiagnosticAction) Read(reader io.Reader) BindingHostDiagnosticAction {
	id := readInt32(reader)
	return BindingHostDiagnosticAction(id)
}

func (FfiConverterBindingHostDiagnosticAction) Write(writer io.Writer, value BindingHostDiagnosticAction) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostDiagnosticAction struct{}

func (_ FfiDestroyerBindingHostDiagnosticAction) Destroy(value BindingHostDiagnosticAction) {
}

type BindingHostDiagnosticEvent interface {
	Destroy()
}
type BindingHostDiagnosticEventBegin struct {
	Action BindingHostDiagnosticAction
}

func (e BindingHostDiagnosticEventBegin) Destroy() {
	FfiDestroyerBindingHostDiagnosticAction{}.Destroy(e.Action)
}

type BindingHostDiagnosticEventFinish struct {
	Token   string
	Outcome BindingHostDiagnosticOutcome
}

func (e BindingHostDiagnosticEventFinish) Destroy() {
	FfiDestroyerString{}.Destroy(e.Token)
	FfiDestroyerBindingHostDiagnosticOutcome{}.Destroy(e.Outcome)
}

type BindingHostDiagnosticEventLifecycle struct {
	State BindingHostLifecycleState
}

func (e BindingHostDiagnosticEventLifecycle) Destroy() {
	FfiDestroyerBindingHostLifecycleState{}.Destroy(e.State)
}

type BindingHostDiagnosticEventNetworkChanged struct {
	Kind      BindingHostNetworkKind
	Available bool
}

func (e BindingHostDiagnosticEventNetworkChanged) Destroy() {
	FfiDestroyerBindingHostNetworkKind{}.Destroy(e.Kind)
	FfiDestroyerBool{}.Destroy(e.Available)
}

type BindingHostDiagnosticEventOwnershipReleased struct {
}

func (e BindingHostDiagnosticEventOwnershipReleased) Destroy() {
}

type FfiConverterBindingHostDiagnosticEvent struct{}

var FfiConverterBindingHostDiagnosticEventINSTANCE = FfiConverterBindingHostDiagnosticEvent{}

func (c FfiConverterBindingHostDiagnosticEvent) Lift(rb RustBufferI) BindingHostDiagnosticEvent {
	return LiftFromRustBuffer[BindingHostDiagnosticEvent](c, rb)
}

func (c FfiConverterBindingHostDiagnosticEvent) Lower(value BindingHostDiagnosticEvent) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticEvent](c, value)
}

func (c FfiConverterBindingHostDiagnosticEvent) LowerExternal(value BindingHostDiagnosticEvent) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticEvent](c, value))
}
func (FfiConverterBindingHostDiagnosticEvent) Read(reader io.Reader) BindingHostDiagnosticEvent {
	id := readInt32(reader)
	switch id {
	case 1:
		return BindingHostDiagnosticEventBegin{
			FfiConverterBindingHostDiagnosticActionINSTANCE.Read(reader),
		}
	case 2:
		return BindingHostDiagnosticEventFinish{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBindingHostDiagnosticOutcomeINSTANCE.Read(reader),
		}
	case 3:
		return BindingHostDiagnosticEventLifecycle{
			FfiConverterBindingHostLifecycleStateINSTANCE.Read(reader),
		}
	case 4:
		return BindingHostDiagnosticEventNetworkChanged{
			FfiConverterBindingHostNetworkKindINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
		}
	case 5:
		return BindingHostDiagnosticEventOwnershipReleased{}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterBindingHostDiagnosticEvent.Read()", id))
	}
}

func (FfiConverterBindingHostDiagnosticEvent) Write(writer io.Writer, value BindingHostDiagnosticEvent) {
	switch variant_value := value.(type) {
	case BindingHostDiagnosticEventBegin:
		writeInt32(writer, 1)
		FfiConverterBindingHostDiagnosticActionINSTANCE.Write(writer, variant_value.Action)
	case BindingHostDiagnosticEventFinish:
		writeInt32(writer, 2)
		FfiConverterStringINSTANCE.Write(writer, variant_value.Token)
		FfiConverterBindingHostDiagnosticOutcomeINSTANCE.Write(writer, variant_value.Outcome)
	case BindingHostDiagnosticEventLifecycle:
		writeInt32(writer, 3)
		FfiConverterBindingHostLifecycleStateINSTANCE.Write(writer, variant_value.State)
	case BindingHostDiagnosticEventNetworkChanged:
		writeInt32(writer, 4)
		FfiConverterBindingHostNetworkKindINSTANCE.Write(writer, variant_value.Kind)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.Available)
	case BindingHostDiagnosticEventOwnershipReleased:
		writeInt32(writer, 5)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterBindingHostDiagnosticEvent.Write", value))
	}
}

type FfiDestroyerBindingHostDiagnosticEvent struct{}

func (_ FfiDestroyerBindingHostDiagnosticEvent) Destroy(value BindingHostDiagnosticEvent) {
	value.Destroy()
}

type BindingHostDiagnosticFailure uint

const (
	BindingHostDiagnosticFailureUnavailable      BindingHostDiagnosticFailure = 1
	BindingHostDiagnosticFailurePermissionDenied BindingHostDiagnosticFailure = 2
	BindingHostDiagnosticFailureLocked           BindingHostDiagnosticFailure = 3
	BindingHostDiagnosticFailureBusy             BindingHostDiagnosticFailure = 4
	BindingHostDiagnosticFailureUnknown          BindingHostDiagnosticFailure = 5
)

type FfiConverterBindingHostDiagnosticFailure struct{}

var FfiConverterBindingHostDiagnosticFailureINSTANCE = FfiConverterBindingHostDiagnosticFailure{}

func (c FfiConverterBindingHostDiagnosticFailure) Lift(rb RustBufferI) BindingHostDiagnosticFailure {
	return LiftFromRustBuffer[BindingHostDiagnosticFailure](c, rb)
}

func (c FfiConverterBindingHostDiagnosticFailure) Lower(value BindingHostDiagnosticFailure) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticFailure](c, value)
}

func (c FfiConverterBindingHostDiagnosticFailure) LowerExternal(value BindingHostDiagnosticFailure) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticFailure](c, value))
}
func (FfiConverterBindingHostDiagnosticFailure) Read(reader io.Reader) BindingHostDiagnosticFailure {
	id := readInt32(reader)
	return BindingHostDiagnosticFailure(id)
}

func (FfiConverterBindingHostDiagnosticFailure) Write(writer io.Writer, value BindingHostDiagnosticFailure) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostDiagnosticFailure struct{}

func (_ FfiDestroyerBindingHostDiagnosticFailure) Destroy(value BindingHostDiagnosticFailure) {
}

type BindingHostDiagnosticOutcome interface {
	Destroy()
}
type BindingHostDiagnosticOutcomeCompleted struct {
}

func (e BindingHostDiagnosticOutcomeCompleted) Destroy() {
}

type BindingHostDiagnosticOutcomeFailed struct {
	Reason BindingHostDiagnosticFailure
}

func (e BindingHostDiagnosticOutcomeFailed) Destroy() {
	FfiDestroyerBindingHostDiagnosticFailure{}.Destroy(e.Reason)
}

type BindingHostDiagnosticOutcomeInterrupted struct {
}

func (e BindingHostDiagnosticOutcomeInterrupted) Destroy() {
}

type FfiConverterBindingHostDiagnosticOutcome struct{}

var FfiConverterBindingHostDiagnosticOutcomeINSTANCE = FfiConverterBindingHostDiagnosticOutcome{}

func (c FfiConverterBindingHostDiagnosticOutcome) Lift(rb RustBufferI) BindingHostDiagnosticOutcome {
	return LiftFromRustBuffer[BindingHostDiagnosticOutcome](c, rb)
}

func (c FfiConverterBindingHostDiagnosticOutcome) Lower(value BindingHostDiagnosticOutcome) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticOutcome](c, value)
}

func (c FfiConverterBindingHostDiagnosticOutcome) LowerExternal(value BindingHostDiagnosticOutcome) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticOutcome](c, value))
}
func (FfiConverterBindingHostDiagnosticOutcome) Read(reader io.Reader) BindingHostDiagnosticOutcome {
	id := readInt32(reader)
	switch id {
	case 1:
		return BindingHostDiagnosticOutcomeCompleted{}
	case 2:
		return BindingHostDiagnosticOutcomeFailed{
			FfiConverterBindingHostDiagnosticFailureINSTANCE.Read(reader),
		}
	case 3:
		return BindingHostDiagnosticOutcomeInterrupted{}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterBindingHostDiagnosticOutcome.Read()", id))
	}
}

func (FfiConverterBindingHostDiagnosticOutcome) Write(writer io.Writer, value BindingHostDiagnosticOutcome) {
	switch variant_value := value.(type) {
	case BindingHostDiagnosticOutcomeCompleted:
		writeInt32(writer, 1)
	case BindingHostDiagnosticOutcomeFailed:
		writeInt32(writer, 2)
		FfiConverterBindingHostDiagnosticFailureINSTANCE.Write(writer, variant_value.Reason)
	case BindingHostDiagnosticOutcomeInterrupted:
		writeInt32(writer, 3)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterBindingHostDiagnosticOutcome.Write", value))
	}
}

type FfiDestroyerBindingHostDiagnosticOutcome struct{}

func (_ FfiDestroyerBindingHostDiagnosticOutcome) Destroy(value BindingHostDiagnosticOutcome) {
	value.Destroy()
}

type BindingHostDiagnosticRecordStatus uint

const (
	BindingHostDiagnosticRecordStatusAccepted         BindingHostDiagnosticRecordStatus = 1
	BindingHostDiagnosticRecordStatusPolicyFiltered   BindingHostDiagnosticRecordStatus = 2
	BindingHostDiagnosticRecordStatusCapacityExceeded BindingHostDiagnosticRecordStatus = 3
	BindingHostDiagnosticRecordStatusInvalidToken     BindingHostDiagnosticRecordStatus = 4
	BindingHostDiagnosticRecordStatusInvalidSource    BindingHostDiagnosticRecordStatus = 5
	BindingHostDiagnosticRecordStatusNotRegistered    BindingHostDiagnosticRecordStatus = 6
	BindingHostDiagnosticRecordStatusUnavailable      BindingHostDiagnosticRecordStatus = 7
	BindingHostDiagnosticRecordStatusAlreadyShutdown  BindingHostDiagnosticRecordStatus = 8
)

type FfiConverterBindingHostDiagnosticRecordStatus struct{}

var FfiConverterBindingHostDiagnosticRecordStatusINSTANCE = FfiConverterBindingHostDiagnosticRecordStatus{}

func (c FfiConverterBindingHostDiagnosticRecordStatus) Lift(rb RustBufferI) BindingHostDiagnosticRecordStatus {
	return LiftFromRustBuffer[BindingHostDiagnosticRecordStatus](c, rb)
}

func (c FfiConverterBindingHostDiagnosticRecordStatus) Lower(value BindingHostDiagnosticRecordStatus) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticRecordStatus](c, value)
}

func (c FfiConverterBindingHostDiagnosticRecordStatus) LowerExternal(value BindingHostDiagnosticRecordStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticRecordStatus](c, value))
}
func (FfiConverterBindingHostDiagnosticRecordStatus) Read(reader io.Reader) BindingHostDiagnosticRecordStatus {
	id := readInt32(reader)
	return BindingHostDiagnosticRecordStatus(id)
}

func (FfiConverterBindingHostDiagnosticRecordStatus) Write(writer io.Writer, value BindingHostDiagnosticRecordStatus) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostDiagnosticRecordStatus struct{}

func (_ FfiDestroyerBindingHostDiagnosticRecordStatus) Destroy(value BindingHostDiagnosticRecordStatus) {
}

type BindingHostDiagnosticSource uint

const (
	BindingHostDiagnosticSourceApplication       BindingHostDiagnosticSource = 1
	BindingHostDiagnosticSourceShareExtension    BindingHostDiagnosticSource = 2
	BindingHostDiagnosticSourceKeyboardExtension BindingHostDiagnosticSource = 3
	BindingHostDiagnosticSourceBackgroundService BindingHostDiagnosticSource = 4
)

type FfiConverterBindingHostDiagnosticSource struct{}

var FfiConverterBindingHostDiagnosticSourceINSTANCE = FfiConverterBindingHostDiagnosticSource{}

func (c FfiConverterBindingHostDiagnosticSource) Lift(rb RustBufferI) BindingHostDiagnosticSource {
	return LiftFromRustBuffer[BindingHostDiagnosticSource](c, rb)
}

func (c FfiConverterBindingHostDiagnosticSource) Lower(value BindingHostDiagnosticSource) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostDiagnosticSource](c, value)
}

func (c FfiConverterBindingHostDiagnosticSource) LowerExternal(value BindingHostDiagnosticSource) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostDiagnosticSource](c, value))
}
func (FfiConverterBindingHostDiagnosticSource) Read(reader io.Reader) BindingHostDiagnosticSource {
	id := readInt32(reader)
	return BindingHostDiagnosticSource(id)
}

func (FfiConverterBindingHostDiagnosticSource) Write(writer io.Writer, value BindingHostDiagnosticSource) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostDiagnosticSource struct{}

func (_ FfiDestroyerBindingHostDiagnosticSource) Destroy(value BindingHostDiagnosticSource) {
}

type BindingHostLifecycleState uint

const (
	BindingHostLifecycleStateForeground BindingHostLifecycleState = 1
	BindingHostLifecycleStateBackground BindingHostLifecycleState = 2
)

type FfiConverterBindingHostLifecycleState struct{}

var FfiConverterBindingHostLifecycleStateINSTANCE = FfiConverterBindingHostLifecycleState{}

func (c FfiConverterBindingHostLifecycleState) Lift(rb RustBufferI) BindingHostLifecycleState {
	return LiftFromRustBuffer[BindingHostLifecycleState](c, rb)
}

func (c FfiConverterBindingHostLifecycleState) Lower(value BindingHostLifecycleState) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostLifecycleState](c, value)
}

func (c FfiConverterBindingHostLifecycleState) LowerExternal(value BindingHostLifecycleState) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostLifecycleState](c, value))
}
func (FfiConverterBindingHostLifecycleState) Read(reader io.Reader) BindingHostLifecycleState {
	id := readInt32(reader)
	return BindingHostLifecycleState(id)
}

func (FfiConverterBindingHostLifecycleState) Write(writer io.Writer, value BindingHostLifecycleState) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostLifecycleState struct{}

func (_ FfiDestroyerBindingHostLifecycleState) Destroy(value BindingHostLifecycleState) {
}

type BindingHostNetworkKind uint

const (
	BindingHostNetworkKindWifi     BindingHostNetworkKind = 1
	BindingHostNetworkKindCellular BindingHostNetworkKind = 2
	BindingHostNetworkKindEthernet BindingHostNetworkKind = 3
	BindingHostNetworkKindOther    BindingHostNetworkKind = 4
	BindingHostNetworkKindUnknown  BindingHostNetworkKind = 5
)

type FfiConverterBindingHostNetworkKind struct{}

var FfiConverterBindingHostNetworkKindINSTANCE = FfiConverterBindingHostNetworkKind{}

func (c FfiConverterBindingHostNetworkKind) Lift(rb RustBufferI) BindingHostNetworkKind {
	return LiftFromRustBuffer[BindingHostNetworkKind](c, rb)
}

func (c FfiConverterBindingHostNetworkKind) Lower(value BindingHostNetworkKind) C.RustBuffer {
	return LowerIntoRustBuffer[BindingHostNetworkKind](c, value)
}

func (c FfiConverterBindingHostNetworkKind) LowerExternal(value BindingHostNetworkKind) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingHostNetworkKind](c, value))
}
func (FfiConverterBindingHostNetworkKind) Read(reader io.Reader) BindingHostNetworkKind {
	id := readInt32(reader)
	return BindingHostNetworkKind(id)
}

func (FfiConverterBindingHostNetworkKind) Write(writer io.Writer, value BindingHostNetworkKind) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingHostNetworkKind struct{}

func (_ FfiDestroyerBindingHostNetworkKind) Destroy(value BindingHostNetworkKind) {
}

type BindingLifecycleAction uint

const (
	BindingLifecycleActionSuspend BindingLifecycleAction = 1
	BindingLifecycleActionResume  BindingLifecycleAction = 2
)

type FfiConverterBindingLifecycleAction struct{}

var FfiConverterBindingLifecycleActionINSTANCE = FfiConverterBindingLifecycleAction{}

func (c FfiConverterBindingLifecycleAction) Lift(rb RustBufferI) BindingLifecycleAction {
	return LiftFromRustBuffer[BindingLifecycleAction](c, rb)
}

func (c FfiConverterBindingLifecycleAction) Lower(value BindingLifecycleAction) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLifecycleAction](c, value)
}

func (c FfiConverterBindingLifecycleAction) LowerExternal(value BindingLifecycleAction) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLifecycleAction](c, value))
}
func (FfiConverterBindingLifecycleAction) Read(reader io.Reader) BindingLifecycleAction {
	id := readInt32(reader)
	return BindingLifecycleAction(id)
}

func (FfiConverterBindingLifecycleAction) Write(writer io.Writer, value BindingLifecycleAction) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingLifecycleAction struct{}

func (_ FfiDestroyerBindingLifecycleAction) Destroy(value BindingLifecycleAction) {
}

type BindingLocalCaptureMode uint

const (
	BindingLocalCaptureModeStandard BindingLocalCaptureMode = 1
	BindingLocalCaptureModeDetailed BindingLocalCaptureMode = 2
)

type FfiConverterBindingLocalCaptureMode struct{}

var FfiConverterBindingLocalCaptureModeINSTANCE = FfiConverterBindingLocalCaptureMode{}

func (c FfiConverterBindingLocalCaptureMode) Lift(rb RustBufferI) BindingLocalCaptureMode {
	return LiftFromRustBuffer[BindingLocalCaptureMode](c, rb)
}

func (c FfiConverterBindingLocalCaptureMode) Lower(value BindingLocalCaptureMode) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLocalCaptureMode](c, value)
}

func (c FfiConverterBindingLocalCaptureMode) LowerExternal(value BindingLocalCaptureMode) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLocalCaptureMode](c, value))
}
func (FfiConverterBindingLocalCaptureMode) Read(reader io.Reader) BindingLocalCaptureMode {
	id := readInt32(reader)
	return BindingLocalCaptureMode(id)
}

func (FfiConverterBindingLocalCaptureMode) Write(writer io.Writer, value BindingLocalCaptureMode) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingLocalCaptureMode struct{}

func (_ FfiDestroyerBindingLocalCaptureMode) Destroy(value BindingLocalCaptureMode) {
}

type BindingLocalDiagnosticError struct {
	err error
}

// Convenience method to turn *BindingLocalDiagnosticError into error
// Avoiding treating nil pointer as non nil error interface
func (err *BindingLocalDiagnosticError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err BindingLocalDiagnosticError) Error() string {
	return fmt.Sprintf("BindingLocalDiagnosticError: %s", err.err.Error())
}

func (err BindingLocalDiagnosticError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrBindingLocalDiagnosticErrorNotInstalled = fmt.Errorf("BindingLocalDiagnosticErrorNotInstalled")
var ErrBindingLocalDiagnosticErrorLocalSinkUnavailable = fmt.Errorf("BindingLocalDiagnosticErrorLocalSinkUnavailable")
var ErrBindingLocalDiagnosticErrorAlreadyShutdown = fmt.Errorf("BindingLocalDiagnosticErrorAlreadyShutdown")
var ErrBindingLocalDiagnosticErrorInvalidDuration = fmt.Errorf("BindingLocalDiagnosticErrorInvalidDuration")
var ErrBindingLocalDiagnosticErrorInvalidCaptureId = fmt.Errorf("BindingLocalDiagnosticErrorInvalidCaptureId")
var ErrBindingLocalDiagnosticErrorInvalidDeadline = fmt.Errorf("BindingLocalDiagnosticErrorInvalidDeadline")
var ErrBindingLocalDiagnosticErrorInvalidHostSource = fmt.Errorf("BindingLocalDiagnosticErrorInvalidHostSource")

// Variant structs
type BindingLocalDiagnosticErrorNotInstalled struct {
}

func NewBindingLocalDiagnosticErrorNotInstalled() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorNotInstalled{}}
}

func (e BindingLocalDiagnosticErrorNotInstalled) destroy() {
}

func (err BindingLocalDiagnosticErrorNotInstalled) Error() string {
	return fmt.Sprint("NotInstalled")
}

func (self BindingLocalDiagnosticErrorNotInstalled) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorNotInstalled
}

type BindingLocalDiagnosticErrorLocalSinkUnavailable struct {
}

func NewBindingLocalDiagnosticErrorLocalSinkUnavailable() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorLocalSinkUnavailable{}}
}

func (e BindingLocalDiagnosticErrorLocalSinkUnavailable) destroy() {
}

func (err BindingLocalDiagnosticErrorLocalSinkUnavailable) Error() string {
	return fmt.Sprint("LocalSinkUnavailable")
}

func (self BindingLocalDiagnosticErrorLocalSinkUnavailable) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorLocalSinkUnavailable
}

type BindingLocalDiagnosticErrorAlreadyShutdown struct {
}

func NewBindingLocalDiagnosticErrorAlreadyShutdown() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorAlreadyShutdown{}}
}

func (e BindingLocalDiagnosticErrorAlreadyShutdown) destroy() {
}

func (err BindingLocalDiagnosticErrorAlreadyShutdown) Error() string {
	return fmt.Sprint("AlreadyShutdown")
}

func (self BindingLocalDiagnosticErrorAlreadyShutdown) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorAlreadyShutdown
}

type BindingLocalDiagnosticErrorInvalidDuration struct {
}

func NewBindingLocalDiagnosticErrorInvalidDuration() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorInvalidDuration{}}
}

func (e BindingLocalDiagnosticErrorInvalidDuration) destroy() {
}

func (err BindingLocalDiagnosticErrorInvalidDuration) Error() string {
	return fmt.Sprint("InvalidDuration")
}

func (self BindingLocalDiagnosticErrorInvalidDuration) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorInvalidDuration
}

type BindingLocalDiagnosticErrorInvalidCaptureId struct {
}

func NewBindingLocalDiagnosticErrorInvalidCaptureId() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorInvalidCaptureId{}}
}

func (e BindingLocalDiagnosticErrorInvalidCaptureId) destroy() {
}

func (err BindingLocalDiagnosticErrorInvalidCaptureId) Error() string {
	return fmt.Sprint("InvalidCaptureId")
}

func (self BindingLocalDiagnosticErrorInvalidCaptureId) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorInvalidCaptureId
}

type BindingLocalDiagnosticErrorInvalidDeadline struct {
}

func NewBindingLocalDiagnosticErrorInvalidDeadline() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorInvalidDeadline{}}
}

func (e BindingLocalDiagnosticErrorInvalidDeadline) destroy() {
}

func (err BindingLocalDiagnosticErrorInvalidDeadline) Error() string {
	return fmt.Sprint("InvalidDeadline")
}

func (self BindingLocalDiagnosticErrorInvalidDeadline) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorInvalidDeadline
}

type BindingLocalDiagnosticErrorInvalidHostSource struct {
}

func NewBindingLocalDiagnosticErrorInvalidHostSource() *BindingLocalDiagnosticError {
	return &BindingLocalDiagnosticError{err: &BindingLocalDiagnosticErrorInvalidHostSource{}}
}

func (e BindingLocalDiagnosticErrorInvalidHostSource) destroy() {
}

func (err BindingLocalDiagnosticErrorInvalidHostSource) Error() string {
	return fmt.Sprint("InvalidHostSource")
}

func (self BindingLocalDiagnosticErrorInvalidHostSource) Is(target error) bool {
	return target == ErrBindingLocalDiagnosticErrorInvalidHostSource
}

type FfiConverterBindingLocalDiagnosticError struct{}

var FfiConverterBindingLocalDiagnosticErrorINSTANCE = FfiConverterBindingLocalDiagnosticError{}

func (c FfiConverterBindingLocalDiagnosticError) Lift(eb RustBufferI) *BindingLocalDiagnosticError {
	return LiftFromRustBuffer[*BindingLocalDiagnosticError](c, eb)
}

func (c FfiConverterBindingLocalDiagnosticError) Lower(value *BindingLocalDiagnosticError) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingLocalDiagnosticError](c, value)
}

func (c FfiConverterBindingLocalDiagnosticError) LowerExternal(value *BindingLocalDiagnosticError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingLocalDiagnosticError](c, value))
}

func (c FfiConverterBindingLocalDiagnosticError) Read(reader io.Reader) *BindingLocalDiagnosticError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorNotInstalled{}}
	case 2:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorLocalSinkUnavailable{}}
	case 3:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorAlreadyShutdown{}}
	case 4:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorInvalidDuration{}}
	case 5:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorInvalidCaptureId{}}
	case 6:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorInvalidDeadline{}}
	case 7:
		return &BindingLocalDiagnosticError{&BindingLocalDiagnosticErrorInvalidHostSource{}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterBindingLocalDiagnosticError.Read()", errorID))
	}
}

func (c FfiConverterBindingLocalDiagnosticError) Write(writer io.Writer, value *BindingLocalDiagnosticError) {
	switch variantValue := value.err.(type) {
	case *BindingLocalDiagnosticErrorNotInstalled:
		writeInt32(writer, 1)
	case *BindingLocalDiagnosticErrorLocalSinkUnavailable:
		writeInt32(writer, 2)
	case *BindingLocalDiagnosticErrorAlreadyShutdown:
		writeInt32(writer, 3)
	case *BindingLocalDiagnosticErrorInvalidDuration:
		writeInt32(writer, 4)
	case *BindingLocalDiagnosticErrorInvalidCaptureId:
		writeInt32(writer, 5)
	case *BindingLocalDiagnosticErrorInvalidDeadline:
		writeInt32(writer, 6)
	case *BindingLocalDiagnosticErrorInvalidHostSource:
		writeInt32(writer, 7)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterBindingLocalDiagnosticError.Write", value))
	}
}

type FfiDestroyerBindingLocalDiagnosticError struct{}

func (_ FfiDestroyerBindingLocalDiagnosticError) Destroy(value *BindingLocalDiagnosticError) {
	switch variantValue := value.err.(type) {
	case BindingLocalDiagnosticErrorNotInstalled:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorLocalSinkUnavailable:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorAlreadyShutdown:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorInvalidDuration:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorInvalidCaptureId:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorInvalidDeadline:
		variantValue.destroy()
	case BindingLocalDiagnosticErrorInvalidHostSource:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerBindingLocalDiagnosticError.Destroy", value))
	}
}

type BindingLocalDiagnosticSource uint

const (
	BindingLocalDiagnosticSourceRuntime               BindingLocalDiagnosticSource = 1
	BindingLocalDiagnosticSourceConnections           BindingLocalDiagnosticSource = 2
	BindingLocalDiagnosticSourceAddressStorage        BindingLocalDiagnosticSource = 3
	BindingLocalDiagnosticSourceDnsDiscovery          BindingLocalDiagnosticSource = 4
	BindingLocalDiagnosticSourceMdnsDiscovery         BindingLocalDiagnosticSource = 5
	BindingLocalDiagnosticSourcePkarrDiscovery        BindingLocalDiagnosticSource = 6
	BindingLocalDiagnosticSourceConnectionPaths       BindingLocalDiagnosticSource = 7
	BindingLocalDiagnosticSourceRelayRecovery         BindingLocalDiagnosticSource = 8
	BindingLocalDiagnosticSourceMembershipUpdates     BindingLocalDiagnosticSource = 9
	BindingLocalDiagnosticSourceSessions              BindingLocalDiagnosticSource = 10
	BindingLocalDiagnosticSourceHostApplication       BindingLocalDiagnosticSource = 11
	BindingLocalDiagnosticSourceHostShareExtension    BindingLocalDiagnosticSource = 12
	BindingLocalDiagnosticSourceHostKeyboardExtension BindingLocalDiagnosticSource = 13
	BindingLocalDiagnosticSourceHostBackgroundService BindingLocalDiagnosticSource = 14
)

type FfiConverterBindingLocalDiagnosticSource struct{}

var FfiConverterBindingLocalDiagnosticSourceINSTANCE = FfiConverterBindingLocalDiagnosticSource{}

func (c FfiConverterBindingLocalDiagnosticSource) Lift(rb RustBufferI) BindingLocalDiagnosticSource {
	return LiftFromRustBuffer[BindingLocalDiagnosticSource](c, rb)
}

func (c FfiConverterBindingLocalDiagnosticSource) Lower(value BindingLocalDiagnosticSource) C.RustBuffer {
	return LowerIntoRustBuffer[BindingLocalDiagnosticSource](c, value)
}

func (c FfiConverterBindingLocalDiagnosticSource) LowerExternal(value BindingLocalDiagnosticSource) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingLocalDiagnosticSource](c, value))
}
func (FfiConverterBindingLocalDiagnosticSource) Read(reader io.Reader) BindingLocalDiagnosticSource {
	id := readInt32(reader)
	return BindingLocalDiagnosticSource(id)
}

func (FfiConverterBindingLocalDiagnosticSource) Write(writer io.Writer, value BindingLocalDiagnosticSource) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingLocalDiagnosticSource struct{}

func (_ FfiDestroyerBindingLocalDiagnosticSource) Destroy(value BindingLocalDiagnosticSource) {
}

// 远端导出器构建失败的阶段；只有固定分类，不含 endpoint 或错误正文。
type BindingObservabilityRemoteSetupFailure uint

const (
	BindingObservabilityRemoteSetupFailureHttpClient    BindingObservabilityRemoteSetupFailure = 1
	BindingObservabilityRemoteSetupFailureTraceExporter BindingObservabilityRemoteSetupFailure = 2
	BindingObservabilityRemoteSetupFailureLogExporter   BindingObservabilityRemoteSetupFailure = 3
)

type FfiConverterBindingObservabilityRemoteSetupFailure struct{}

var FfiConverterBindingObservabilityRemoteSetupFailureINSTANCE = FfiConverterBindingObservabilityRemoteSetupFailure{}

func (c FfiConverterBindingObservabilityRemoteSetupFailure) Lift(rb RustBufferI) BindingObservabilityRemoteSetupFailure {
	return LiftFromRustBuffer[BindingObservabilityRemoteSetupFailure](c, rb)
}

func (c FfiConverterBindingObservabilityRemoteSetupFailure) Lower(value BindingObservabilityRemoteSetupFailure) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilityRemoteSetupFailure](c, value)
}

func (c FfiConverterBindingObservabilityRemoteSetupFailure) LowerExternal(value BindingObservabilityRemoteSetupFailure) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilityRemoteSetupFailure](c, value))
}
func (FfiConverterBindingObservabilityRemoteSetupFailure) Read(reader io.Reader) BindingObservabilityRemoteSetupFailure {
	id := readInt32(reader)
	return BindingObservabilityRemoteSetupFailure(id)
}

func (FfiConverterBindingObservabilityRemoteSetupFailure) Write(writer io.Writer, value BindingObservabilityRemoteSetupFailure) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingObservabilityRemoteSetupFailure struct{}

func (_ FfiDestroyerBindingObservabilityRemoteSetupFailure) Destroy(value BindingObservabilityRemoteSetupFailure) {
}

type BindingObservabilitySetupStatus uint

const (
	BindingObservabilitySetupStatusDisabled    BindingObservabilitySetupStatus = 1
	BindingObservabilitySetupStatusReady       BindingObservabilitySetupStatus = 2
	BindingObservabilitySetupStatusUnavailable BindingObservabilitySetupStatus = 3
)

type FfiConverterBindingObservabilitySetupStatus struct{}

var FfiConverterBindingObservabilitySetupStatusINSTANCE = FfiConverterBindingObservabilitySetupStatus{}

func (c FfiConverterBindingObservabilitySetupStatus) Lift(rb RustBufferI) BindingObservabilitySetupStatus {
	return LiftFromRustBuffer[BindingObservabilitySetupStatus](c, rb)
}

func (c FfiConverterBindingObservabilitySetupStatus) Lower(value BindingObservabilitySetupStatus) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilitySetupStatus](c, value)
}

func (c FfiConverterBindingObservabilitySetupStatus) LowerExternal(value BindingObservabilitySetupStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilitySetupStatus](c, value))
}
func (FfiConverterBindingObservabilitySetupStatus) Read(reader io.Reader) BindingObservabilitySetupStatus {
	id := readInt32(reader)
	return BindingObservabilitySetupStatus(id)
}

func (FfiConverterBindingObservabilitySetupStatus) Write(writer io.Writer, value BindingObservabilitySetupStatus) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingObservabilitySetupStatus struct{}

func (_ FfiDestroyerBindingObservabilitySetupStatus) Destroy(value BindingObservabilitySetupStatus) {
}

type BindingObservabilitySignalResult uint

const (
	BindingObservabilitySignalResultCompleted       BindingObservabilitySignalResult = 1
	BindingObservabilitySignalResultFailed          BindingObservabilitySignalResult = 2
	BindingObservabilitySignalResultTimedOut        BindingObservabilitySignalResult = 3
	BindingObservabilitySignalResultAlreadyShutdown BindingObservabilitySignalResult = 4
)

type FfiConverterBindingObservabilitySignalResult struct{}

var FfiConverterBindingObservabilitySignalResultINSTANCE = FfiConverterBindingObservabilitySignalResult{}

func (c FfiConverterBindingObservabilitySignalResult) Lift(rb RustBufferI) BindingObservabilitySignalResult {
	return LiftFromRustBuffer[BindingObservabilitySignalResult](c, rb)
}

func (c FfiConverterBindingObservabilitySignalResult) Lower(value BindingObservabilitySignalResult) C.RustBuffer {
	return LowerIntoRustBuffer[BindingObservabilitySignalResult](c, value)
}

func (c FfiConverterBindingObservabilitySignalResult) LowerExternal(value BindingObservabilitySignalResult) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingObservabilitySignalResult](c, value))
}
func (FfiConverterBindingObservabilitySignalResult) Read(reader io.Reader) BindingObservabilitySignalResult {
	id := readInt32(reader)
	return BindingObservabilitySignalResult(id)
}

func (FfiConverterBindingObservabilitySignalResult) Write(writer io.Writer, value BindingObservabilitySignalResult) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingObservabilitySignalResult struct{}

func (_ FfiDestroyerBindingObservabilitySignalResult) Destroy(value BindingObservabilitySignalResult) {
}

type BindingOperationTerminal uint

const (
	BindingOperationTerminalSucceeded BindingOperationTerminal = 1
	BindingOperationTerminalFailed    BindingOperationTerminal = 2
	BindingOperationTerminalCancelled BindingOperationTerminal = 3
)

type FfiConverterBindingOperationTerminal struct{}

var FfiConverterBindingOperationTerminalINSTANCE = FfiConverterBindingOperationTerminal{}

func (c FfiConverterBindingOperationTerminal) Lift(rb RustBufferI) BindingOperationTerminal {
	return LiftFromRustBuffer[BindingOperationTerminal](c, rb)
}

func (c FfiConverterBindingOperationTerminal) Lower(value BindingOperationTerminal) C.RustBuffer {
	return LowerIntoRustBuffer[BindingOperationTerminal](c, value)
}

func (c FfiConverterBindingOperationTerminal) LowerExternal(value BindingOperationTerminal) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingOperationTerminal](c, value))
}
func (FfiConverterBindingOperationTerminal) Read(reader io.Reader) BindingOperationTerminal {
	id := readInt32(reader)
	return BindingOperationTerminal(id)
}

func (FfiConverterBindingOperationTerminal) Write(writer io.Writer, value BindingOperationTerminal) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingOperationTerminal struct{}

func (_ FfiDestroyerBindingOperationTerminal) Destroy(value BindingOperationTerminal) {
}

type BindingRePairingScope uint

const (
	BindingRePairingScopeAllDevices BindingRePairingScope = 1
)

type FfiConverterBindingRePairingScope struct{}

var FfiConverterBindingRePairingScopeINSTANCE = FfiConverterBindingRePairingScope{}

func (c FfiConverterBindingRePairingScope) Lift(rb RustBufferI) BindingRePairingScope {
	return LiftFromRustBuffer[BindingRePairingScope](c, rb)
}

func (c FfiConverterBindingRePairingScope) Lower(value BindingRePairingScope) C.RustBuffer {
	return LowerIntoRustBuffer[BindingRePairingScope](c, value)
}

func (c FfiConverterBindingRePairingScope) LowerExternal(value BindingRePairingScope) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingRePairingScope](c, value))
}
func (FfiConverterBindingRePairingScope) Read(reader io.Reader) BindingRePairingScope {
	id := readInt32(reader)
	return BindingRePairingScope(id)
}

func (FfiConverterBindingRePairingScope) Write(writer io.Writer, value BindingRePairingScope) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingRePairingScope struct{}

func (_ FfiDestroyerBindingRePairingScope) Destroy(value BindingRePairingScope) {
}

type BindingRefreshReason uint

const (
	BindingRefreshReasonConsumerLagged   BindingRefreshReason = 1
	BindingRefreshReasonStateInvalidated BindingRefreshReason = 2
)

type FfiConverterBindingRefreshReason struct{}

var FfiConverterBindingRefreshReasonINSTANCE = FfiConverterBindingRefreshReason{}

func (c FfiConverterBindingRefreshReason) Lift(rb RustBufferI) BindingRefreshReason {
	return LiftFromRustBuffer[BindingRefreshReason](c, rb)
}

func (c FfiConverterBindingRefreshReason) Lower(value BindingRefreshReason) C.RustBuffer {
	return LowerIntoRustBuffer[BindingRefreshReason](c, value)
}

func (c FfiConverterBindingRefreshReason) LowerExternal(value BindingRefreshReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingRefreshReason](c, value))
}
func (FfiConverterBindingRefreshReason) Read(reader io.Reader) BindingRefreshReason {
	id := readInt32(reader)
	return BindingRefreshReason(id)
}

func (FfiConverterBindingRefreshReason) Write(writer io.Writer, value BindingRefreshReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingRefreshReason struct{}

func (_ FfiDestroyerBindingRefreshReason) Destroy(value BindingRefreshReason) {
}

type BindingSourceCapability uint

const (
	BindingSourceCapabilitySupported   BindingSourceCapability = 1
	BindingSourceCapabilityPartial     BindingSourceCapability = 2
	BindingSourceCapabilityUnsupported BindingSourceCapability = 3
	BindingSourceCapabilityUnknown     BindingSourceCapability = 4
)

type FfiConverterBindingSourceCapability struct{}

var FfiConverterBindingSourceCapabilityINSTANCE = FfiConverterBindingSourceCapability{}

func (c FfiConverterBindingSourceCapability) Lift(rb RustBufferI) BindingSourceCapability {
	return LiftFromRustBuffer[BindingSourceCapability](c, rb)
}

func (c FfiConverterBindingSourceCapability) Lower(value BindingSourceCapability) C.RustBuffer {
	return LowerIntoRustBuffer[BindingSourceCapability](c, value)
}

func (c FfiConverterBindingSourceCapability) LowerExternal(value BindingSourceCapability) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingSourceCapability](c, value))
}
func (FfiConverterBindingSourceCapability) Read(reader io.Reader) BindingSourceCapability {
	id := readInt32(reader)
	return BindingSourceCapability(id)
}

func (FfiConverterBindingSourceCapability) Write(writer io.Writer, value BindingSourceCapability) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingSourceCapability struct{}

func (_ FfiDestroyerBindingSourceCapability) Destroy(value BindingSourceCapability) {
}

type BindingSourceCollection uint

const (
	BindingSourceCollectionEnabled       BindingSourceCollection = 1
	BindingSourceCollectionDisabled      BindingSourceCollection = 2
	BindingSourceCollectionUnavailable   BindingSourceCollection = 3
	BindingSourceCollectionNotRegistered BindingSourceCollection = 4
)

type FfiConverterBindingSourceCollection struct{}

var FfiConverterBindingSourceCollectionINSTANCE = FfiConverterBindingSourceCollection{}

func (c FfiConverterBindingSourceCollection) Lift(rb RustBufferI) BindingSourceCollection {
	return LiftFromRustBuffer[BindingSourceCollection](c, rb)
}

func (c FfiConverterBindingSourceCollection) Lower(value BindingSourceCollection) C.RustBuffer {
	return LowerIntoRustBuffer[BindingSourceCollection](c, value)
}

func (c FfiConverterBindingSourceCollection) LowerExternal(value BindingSourceCollection) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingSourceCollection](c, value))
}
func (FfiConverterBindingSourceCollection) Read(reader io.Reader) BindingSourceCollection {
	id := readInt32(reader)
	return BindingSourceCollection(id)
}

func (FfiConverterBindingSourceCollection) Write(writer io.Writer, value BindingSourceCollection) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingSourceCollection struct{}

func (_ FfiDestroyerBindingSourceCollection) Destroy(value BindingSourceCollection) {
}

type BindingStopCaptureResult uint

const (
	BindingStopCaptureResultStopped          BindingStopCaptureResult = 1
	BindingStopCaptureResultAlreadyStopped   BindingStopCaptureResult = 2
	BindingStopCaptureResultDifferentCapture BindingStopCaptureResult = 3
)

type FfiConverterBindingStopCaptureResult struct{}

var FfiConverterBindingStopCaptureResultINSTANCE = FfiConverterBindingStopCaptureResult{}

func (c FfiConverterBindingStopCaptureResult) Lift(rb RustBufferI) BindingStopCaptureResult {
	return LiftFromRustBuffer[BindingStopCaptureResult](c, rb)
}

func (c FfiConverterBindingStopCaptureResult) Lower(value BindingStopCaptureResult) C.RustBuffer {
	return LowerIntoRustBuffer[BindingStopCaptureResult](c, value)
}

func (c FfiConverterBindingStopCaptureResult) LowerExternal(value BindingStopCaptureResult) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingStopCaptureResult](c, value))
}
func (FfiConverterBindingStopCaptureResult) Read(reader io.Reader) BindingStopCaptureResult {
	id := readInt32(reader)
	return BindingStopCaptureResult(id)
}

func (FfiConverterBindingStopCaptureResult) Write(writer io.Writer, value BindingStopCaptureResult) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingStopCaptureResult struct{}

func (_ FfiDestroyerBindingStopCaptureResult) Destroy(value BindingStopCaptureResult) {
}

type BindingTransferDirection uint

const (
	BindingTransferDirectionSending   BindingTransferDirection = 1
	BindingTransferDirectionReceiving BindingTransferDirection = 2
)

type FfiConverterBindingTransferDirection struct{}

var FfiConverterBindingTransferDirectionINSTANCE = FfiConverterBindingTransferDirection{}

func (c FfiConverterBindingTransferDirection) Lift(rb RustBufferI) BindingTransferDirection {
	return LiftFromRustBuffer[BindingTransferDirection](c, rb)
}

func (c FfiConverterBindingTransferDirection) Lower(value BindingTransferDirection) C.RustBuffer {
	return LowerIntoRustBuffer[BindingTransferDirection](c, value)
}

func (c FfiConverterBindingTransferDirection) LowerExternal(value BindingTransferDirection) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[BindingTransferDirection](c, value))
}
func (FfiConverterBindingTransferDirection) Read(reader io.Reader) BindingTransferDirection {
	id := readInt32(reader)
	return BindingTransferDirection(id)
}

func (FfiConverterBindingTransferDirection) Write(writer io.Writer, value BindingTransferDirection) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerBindingTransferDirection struct{}

func (_ FfiDestroyerBindingTransferDirection) Destroy(value BindingTransferDirection) {
}

type ConnectivityOpportunity uint

const (
	ConnectivityOpportunityForeground     ConnectivityOpportunity = 1
	ConnectivityOpportunitySystemWake     ConnectivityOpportunity = 2
	ConnectivityOpportunityNetworkChanged ConnectivityOpportunity = 3
)

type FfiConverterConnectivityOpportunity struct{}

var FfiConverterConnectivityOpportunityINSTANCE = FfiConverterConnectivityOpportunity{}

func (c FfiConverterConnectivityOpportunity) Lift(rb RustBufferI) ConnectivityOpportunity {
	return LiftFromRustBuffer[ConnectivityOpportunity](c, rb)
}

func (c FfiConverterConnectivityOpportunity) Lower(value ConnectivityOpportunity) C.RustBuffer {
	return LowerIntoRustBuffer[ConnectivityOpportunity](c, value)
}

func (c FfiConverterConnectivityOpportunity) LowerExternal(value ConnectivityOpportunity) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ConnectivityOpportunity](c, value))
}
func (FfiConverterConnectivityOpportunity) Read(reader io.Reader) ConnectivityOpportunity {
	id := readInt32(reader)
	return ConnectivityOpportunity(id)
}

func (FfiConverterConnectivityOpportunity) Write(writer io.Writer, value ConnectivityOpportunity) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerConnectivityOpportunity struct{}

func (_ FfiDestroyerConnectivityOpportunity) Destroy(value ConnectivityOpportunity) {
}

type CustomRelayMutationRejection uint

const (
	CustomRelayMutationRejectionInvalidUrl CustomRelayMutationRejection = 1
	CustomRelayMutationRejectionDuplicate  CustomRelayMutationRejection = 2
	CustomRelayMutationRejectionNotFound   CustomRelayMutationRejection = 3
)

type FfiConverterCustomRelayMutationRejection struct{}

var FfiConverterCustomRelayMutationRejectionINSTANCE = FfiConverterCustomRelayMutationRejection{}

func (c FfiConverterCustomRelayMutationRejection) Lift(rb RustBufferI) CustomRelayMutationRejection {
	return LiftFromRustBuffer[CustomRelayMutationRejection](c, rb)
}

func (c FfiConverterCustomRelayMutationRejection) Lower(value CustomRelayMutationRejection) C.RustBuffer {
	return LowerIntoRustBuffer[CustomRelayMutationRejection](c, value)
}

func (c FfiConverterCustomRelayMutationRejection) LowerExternal(value CustomRelayMutationRejection) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CustomRelayMutationRejection](c, value))
}
func (FfiConverterCustomRelayMutationRejection) Read(reader io.Reader) CustomRelayMutationRejection {
	id := readInt32(reader)
	return CustomRelayMutationRejection(id)
}

func (FfiConverterCustomRelayMutationRejection) Write(writer io.Writer, value CustomRelayMutationRejection) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerCustomRelayMutationRejection struct{}

func (_ FfiDestroyerCustomRelayMutationRejection) Destroy(value CustomRelayMutationRejection) {
}

type DeviceTrustChoice uint

const (
	DeviceTrustChoiceApplyChange            DeviceTrustChoice = 1
	DeviceTrustChoiceKeepCurrentDeviceGroup DeviceTrustChoice = 2
)

type FfiConverterDeviceTrustChoice struct{}

var FfiConverterDeviceTrustChoiceINSTANCE = FfiConverterDeviceTrustChoice{}

func (c FfiConverterDeviceTrustChoice) Lift(rb RustBufferI) DeviceTrustChoice {
	return LiftFromRustBuffer[DeviceTrustChoice](c, rb)
}

func (c FfiConverterDeviceTrustChoice) Lower(value DeviceTrustChoice) C.RustBuffer {
	return LowerIntoRustBuffer[DeviceTrustChoice](c, value)
}

func (c FfiConverterDeviceTrustChoice) LowerExternal(value DeviceTrustChoice) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[DeviceTrustChoice](c, value))
}
func (FfiConverterDeviceTrustChoice) Read(reader io.Reader) DeviceTrustChoice {
	id := readInt32(reader)
	return DeviceTrustChoice(id)
}

func (FfiConverterDeviceTrustChoice) Write(writer io.Writer, value DeviceTrustChoice) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerDeviceTrustChoice struct{}

func (_ FfiDestroyerDeviceTrustChoice) Destroy(value DeviceTrustChoice) {
}

type EntryNotResendableReason uint

const (
	EntryNotResendableReasonRemoteOrigin EntryNotResendableReason = 1
	EntryNotResendableReasonPayloadLost  EntryNotResendableReason = 2
)

type FfiConverterEntryNotResendableReason struct{}

var FfiConverterEntryNotResendableReasonINSTANCE = FfiConverterEntryNotResendableReason{}

func (c FfiConverterEntryNotResendableReason) Lift(rb RustBufferI) EntryNotResendableReason {
	return LiftFromRustBuffer[EntryNotResendableReason](c, rb)
}

func (c FfiConverterEntryNotResendableReason) Lower(value EntryNotResendableReason) C.RustBuffer {
	return LowerIntoRustBuffer[EntryNotResendableReason](c, value)
}

func (c FfiConverterEntryNotResendableReason) LowerExternal(value EntryNotResendableReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[EntryNotResendableReason](c, value))
}
func (FfiConverterEntryNotResendableReason) Read(reader io.Reader) EntryNotResendableReason {
	id := readInt32(reader)
	return EntryNotResendableReason(id)
}

func (FfiConverterEntryNotResendableReason) Write(writer io.Writer, value EntryNotResendableReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerEntryNotResendableReason struct{}

func (_ FfiDestroyerEntryNotResendableReason) Destroy(value EntryNotResendableReason) {
}

type HostBindingError struct {
	err error
}

// Convenience method to turn *HostBindingError into error
// Avoiding treating nil pointer as non nil error interface
func (err *HostBindingError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err HostBindingError) Error() string {
	return fmt.Sprintf("HostBindingError: %s", err.err.Error())
}

func (err HostBindingError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrHostBindingErrorUnavailable = fmt.Errorf("HostBindingErrorUnavailable")
var ErrHostBindingErrorPermissionDenied = fmt.Errorf("HostBindingErrorPermissionDenied")
var ErrHostBindingErrorInvalidHandle = fmt.Errorf("HostBindingErrorInvalidHandle")
var ErrHostBindingErrorIo = fmt.Errorf("HostBindingErrorIo")

// Variant structs
type HostBindingErrorUnavailable struct {
}

func NewHostBindingErrorUnavailable() *HostBindingError {
	return &HostBindingError{err: &HostBindingErrorUnavailable{}}
}

func (e HostBindingErrorUnavailable) destroy() {
}

func (err HostBindingErrorUnavailable) Error() string {
	return fmt.Sprint("Unavailable")
}

func (self HostBindingErrorUnavailable) Is(target error) bool {
	return target == ErrHostBindingErrorUnavailable
}

type HostBindingErrorPermissionDenied struct {
}

func NewHostBindingErrorPermissionDenied() *HostBindingError {
	return &HostBindingError{err: &HostBindingErrorPermissionDenied{}}
}

func (e HostBindingErrorPermissionDenied) destroy() {
}

func (err HostBindingErrorPermissionDenied) Error() string {
	return fmt.Sprint("PermissionDenied")
}

func (self HostBindingErrorPermissionDenied) Is(target error) bool {
	return target == ErrHostBindingErrorPermissionDenied
}

type HostBindingErrorInvalidHandle struct {
}

func NewHostBindingErrorInvalidHandle() *HostBindingError {
	return &HostBindingError{err: &HostBindingErrorInvalidHandle{}}
}

func (e HostBindingErrorInvalidHandle) destroy() {
}

func (err HostBindingErrorInvalidHandle) Error() string {
	return fmt.Sprint("InvalidHandle")
}

func (self HostBindingErrorInvalidHandle) Is(target error) bool {
	return target == ErrHostBindingErrorInvalidHandle
}

type HostBindingErrorIo struct {
}

func NewHostBindingErrorIo() *HostBindingError {
	return &HostBindingError{err: &HostBindingErrorIo{}}
}

func (e HostBindingErrorIo) destroy() {
}

func (err HostBindingErrorIo) Error() string {
	return fmt.Sprint("Io")
}

func (self HostBindingErrorIo) Is(target error) bool {
	return target == ErrHostBindingErrorIo
}

type FfiConverterHostBindingError struct{}

var FfiConverterHostBindingErrorINSTANCE = FfiConverterHostBindingError{}

func (c FfiConverterHostBindingError) Lift(eb RustBufferI) *HostBindingError {
	return LiftFromRustBuffer[*HostBindingError](c, eb)
}

func (c FfiConverterHostBindingError) Lower(value *HostBindingError) C.RustBuffer {
	return LowerIntoRustBuffer[*HostBindingError](c, value)
}

func (c FfiConverterHostBindingError) LowerExternal(value *HostBindingError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*HostBindingError](c, value))
}

func (c FfiConverterHostBindingError) Read(reader io.Reader) *HostBindingError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &HostBindingError{&HostBindingErrorUnavailable{}}
	case 2:
		return &HostBindingError{&HostBindingErrorPermissionDenied{}}
	case 3:
		return &HostBindingError{&HostBindingErrorInvalidHandle{}}
	case 4:
		return &HostBindingError{&HostBindingErrorIo{}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterHostBindingError.Read()", errorID))
	}
}

func (c FfiConverterHostBindingError) Write(writer io.Writer, value *HostBindingError) {
	switch variantValue := value.err.(type) {
	case *HostBindingErrorUnavailable:
		writeInt32(writer, 1)
	case *HostBindingErrorPermissionDenied:
		writeInt32(writer, 2)
	case *HostBindingErrorInvalidHandle:
		writeInt32(writer, 3)
	case *HostBindingErrorIo:
		writeInt32(writer, 4)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterHostBindingError.Write", value))
	}
}

type FfiDestroyerHostBindingError struct{}

func (_ FfiDestroyerHostBindingError) Destroy(value *HostBindingError) {
	switch variantValue := value.err.(type) {
	case HostBindingErrorUnavailable:
		variantValue.destroy()
	case HostBindingErrorPermissionDenied:
		variantValue.destroy()
	case HostBindingErrorInvalidHandle:
		variantValue.destroy()
	case HostBindingErrorIo:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerHostBindingError.Destroy", value))
	}
}

type InvitationAvailability uint

const (
	InvitationAvailabilityCrossNetwork     InvitationAvailability = 1
	InvitationAvailabilitySameLocalNetwork InvitationAvailability = 2
)

type FfiConverterInvitationAvailability struct{}

var FfiConverterInvitationAvailabilityINSTANCE = FfiConverterInvitationAvailability{}

func (c FfiConverterInvitationAvailability) Lift(rb RustBufferI) InvitationAvailability {
	return LiftFromRustBuffer[InvitationAvailability](c, rb)
}

func (c FfiConverterInvitationAvailability) Lower(value InvitationAvailability) C.RustBuffer {
	return LowerIntoRustBuffer[InvitationAvailability](c, value)
}

func (c FfiConverterInvitationAvailability) LowerExternal(value InvitationAvailability) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[InvitationAvailability](c, value))
}
func (FfiConverterInvitationAvailability) Read(reader io.Reader) InvitationAvailability {
	id := readInt32(reader)
	return InvitationAvailability(id)
}

func (FfiConverterInvitationAvailability) Write(writer io.Writer, value InvitationAvailability) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerInvitationAvailability struct{}

func (_ FfiDestroyerInvitationAvailability) Destroy(value InvitationAvailability) {
}

type JoinSpaceAttentionReason uint

const (
	JoinSpaceAttentionReasonOutcomeCannotBeProven   JoinSpaceAttentionReason = 1
	JoinSpaceAttentionReasonContinuationUnavailable JoinSpaceAttentionReason = 2
)

type FfiConverterJoinSpaceAttentionReason struct{}

var FfiConverterJoinSpaceAttentionReasonINSTANCE = FfiConverterJoinSpaceAttentionReason{}

func (c FfiConverterJoinSpaceAttentionReason) Lift(rb RustBufferI) JoinSpaceAttentionReason {
	return LiftFromRustBuffer[JoinSpaceAttentionReason](c, rb)
}

func (c FfiConverterJoinSpaceAttentionReason) Lower(value JoinSpaceAttentionReason) C.RustBuffer {
	return LowerIntoRustBuffer[JoinSpaceAttentionReason](c, value)
}

func (c FfiConverterJoinSpaceAttentionReason) LowerExternal(value JoinSpaceAttentionReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinSpaceAttentionReason](c, value))
}
func (FfiConverterJoinSpaceAttentionReason) Read(reader io.Reader) JoinSpaceAttentionReason {
	id := readInt32(reader)
	return JoinSpaceAttentionReason(id)
}

func (FfiConverterJoinSpaceAttentionReason) Write(writer io.Writer, value JoinSpaceAttentionReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerJoinSpaceAttentionReason struct{}

func (_ FfiDestroyerJoinSpaceAttentionReason) Destroy(value JoinSpaceAttentionReason) {
}

type JoinSpaceAttentionRecovery uint

const (
	JoinSpaceAttentionRecoveryPreserveDataAndContactSupport JoinSpaceAttentionRecovery = 1
	JoinSpaceAttentionRecoveryRestartWithNewInvitation      JoinSpaceAttentionRecovery = 2
)

type FfiConverterJoinSpaceAttentionRecovery struct{}

var FfiConverterJoinSpaceAttentionRecoveryINSTANCE = FfiConverterJoinSpaceAttentionRecovery{}

func (c FfiConverterJoinSpaceAttentionRecovery) Lift(rb RustBufferI) JoinSpaceAttentionRecovery {
	return LiftFromRustBuffer[JoinSpaceAttentionRecovery](c, rb)
}

func (c FfiConverterJoinSpaceAttentionRecovery) Lower(value JoinSpaceAttentionRecovery) C.RustBuffer {
	return LowerIntoRustBuffer[JoinSpaceAttentionRecovery](c, value)
}

func (c FfiConverterJoinSpaceAttentionRecovery) LowerExternal(value JoinSpaceAttentionRecovery) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinSpaceAttentionRecovery](c, value))
}
func (FfiConverterJoinSpaceAttentionRecovery) Read(reader io.Reader) JoinSpaceAttentionRecovery {
	id := readInt32(reader)
	return JoinSpaceAttentionRecovery(id)
}

func (FfiConverterJoinSpaceAttentionRecovery) Write(writer io.Writer, value JoinSpaceAttentionRecovery) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerJoinSpaceAttentionRecovery struct{}

func (_ FfiDestroyerJoinSpaceAttentionRecovery) Destroy(value JoinSpaceAttentionRecovery) {
}

type JoinSpaceRejectionReason uint

const (
	JoinSpaceRejectionReasonInvitationUnavailable    JoinSpaceRejectionReason = 1
	JoinSpaceRejectionReasonAuthenticationRejected   JoinSpaceRejectionReason = 2
	JoinSpaceRejectionReasonIdentityConflict         JoinSpaceRejectionReason = 3
	JoinSpaceRejectionReasonBaseHistoryChanged       JoinSpaceRejectionReason = 4
	JoinSpaceRejectionReasonJoinerHistoryAhead       JoinSpaceRejectionReason = 5
	JoinSpaceRejectionReasonHistoryConflict          JoinSpaceRejectionReason = 6
	JoinSpaceRejectionReasonCompletionInvalid        JoinSpaceRejectionReason = 7
	JoinSpaceRejectionReasonMembershipHistoryInvalid JoinSpaceRejectionReason = 8
	JoinSpaceRejectionReasonSecurityMaterialInvalid  JoinSpaceRejectionReason = 9
	JoinSpaceRejectionReasonRelationshipConflict     JoinSpaceRejectionReason = 10
	JoinSpaceRejectionReasonActivationStateInvalid   JoinSpaceRejectionReason = 11
	JoinSpaceRejectionReasonPeerUpgradeRequired      JoinSpaceRejectionReason = 12
	JoinSpaceRejectionReasonCancelled                JoinSpaceRejectionReason = 13
	JoinSpaceRejectionReasonRemovedBeforeActivation  JoinSpaceRejectionReason = 14
)

type FfiConverterJoinSpaceRejectionReason struct{}

var FfiConverterJoinSpaceRejectionReasonINSTANCE = FfiConverterJoinSpaceRejectionReason{}

func (c FfiConverterJoinSpaceRejectionReason) Lift(rb RustBufferI) JoinSpaceRejectionReason {
	return LiftFromRustBuffer[JoinSpaceRejectionReason](c, rb)
}

func (c FfiConverterJoinSpaceRejectionReason) Lower(value JoinSpaceRejectionReason) C.RustBuffer {
	return LowerIntoRustBuffer[JoinSpaceRejectionReason](c, value)
}

func (c FfiConverterJoinSpaceRejectionReason) LowerExternal(value JoinSpaceRejectionReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinSpaceRejectionReason](c, value))
}
func (FfiConverterJoinSpaceRejectionReason) Read(reader io.Reader) JoinSpaceRejectionReason {
	id := readInt32(reader)
	return JoinSpaceRejectionReason(id)
}

func (FfiConverterJoinSpaceRejectionReason) Write(writer io.Writer, value JoinSpaceRejectionReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerJoinSpaceRejectionReason struct{}

func (_ FfiDestroyerJoinSpaceRejectionReason) Destroy(value JoinSpaceRejectionReason) {
}

type JoinSpaceStatus interface {
	Destroy()
}
type JoinSpaceStatusActive struct {
	JoinId              string
	JoinedSpace         JoinedSpace
	PeerUpgradeRequired bool
}

func (e JoinSpaceStatusActive) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerJoinedSpace{}.Destroy(e.JoinedSpace)
	FfiDestroyerBool{}.Destroy(e.PeerUpgradeRequired)
}

type JoinSpaceStatusPending struct {
	JoinId                     string
	TargetSpaceId              *string
	SponsorDeviceId            *string
	SponsorIdentityFingerprint *string
	CancelRequested            bool
	PeerUpgradeRequired        bool
}

func (e JoinSpaceStatusPending) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerOptionalString{}.Destroy(e.TargetSpaceId)
	FfiDestroyerOptionalString{}.Destroy(e.SponsorDeviceId)
	FfiDestroyerOptionalString{}.Destroy(e.SponsorIdentityFingerprint)
	FfiDestroyerBool{}.Destroy(e.CancelRequested)
	FfiDestroyerBool{}.Destroy(e.PeerUpgradeRequired)
}

type JoinSpaceStatusProcessing struct {
	JoinId                     string
	TargetSpaceId              string
	SponsorDeviceId            string
	SponsorIdentityFingerprint string
	PeerUpgradeRequired        bool
}

func (e JoinSpaceStatusProcessing) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerString{}.Destroy(e.TargetSpaceId)
	FfiDestroyerString{}.Destroy(e.SponsorDeviceId)
	FfiDestroyerString{}.Destroy(e.SponsorIdentityFingerprint)
	FfiDestroyerBool{}.Destroy(e.PeerUpgradeRequired)
}

type JoinSpaceStatusNeedsAttention struct {
	JoinId        string
	Reason        JoinSpaceAttentionReason
	Recovery      JoinSpaceAttentionRecovery
	NextRetryAtMs *int64
}

func (e JoinSpaceStatusNeedsAttention) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerJoinSpaceAttentionReason{}.Destroy(e.Reason)
	FfiDestroyerJoinSpaceAttentionRecovery{}.Destroy(e.Recovery)
	FfiDestroyerOptionalInt64{}.Destroy(e.NextRetryAtMs)
}

type JoinSpaceStatusRejected struct {
	JoinId string
	Reason JoinSpaceRejectionReason
}

func (e JoinSpaceStatusRejected) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerJoinSpaceRejectionReason{}.Destroy(e.Reason)
}

type JoinSpaceStatusTerminated struct {
	JoinId string
	Reason JoinSpaceTerminationReason
}

func (e JoinSpaceStatusTerminated) Destroy() {
	FfiDestroyerString{}.Destroy(e.JoinId)
	FfiDestroyerJoinSpaceTerminationReason{}.Destroy(e.Reason)
}

type FfiConverterJoinSpaceStatus struct{}

var FfiConverterJoinSpaceStatusINSTANCE = FfiConverterJoinSpaceStatus{}

func (c FfiConverterJoinSpaceStatus) Lift(rb RustBufferI) JoinSpaceStatus {
	return LiftFromRustBuffer[JoinSpaceStatus](c, rb)
}

func (c FfiConverterJoinSpaceStatus) Lower(value JoinSpaceStatus) C.RustBuffer {
	return LowerIntoRustBuffer[JoinSpaceStatus](c, value)
}

func (c FfiConverterJoinSpaceStatus) LowerExternal(value JoinSpaceStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinSpaceStatus](c, value))
}
func (FfiConverterJoinSpaceStatus) Read(reader io.Reader) JoinSpaceStatus {
	id := readInt32(reader)
	switch id {
	case 1:
		return JoinSpaceStatusActive{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterJoinedSpaceINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
		}
	case 2:
		return JoinSpaceStatusPending{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterOptionalStringINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
		}
	case 3:
		return JoinSpaceStatusProcessing{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterBoolINSTANCE.Read(reader),
		}
	case 4:
		return JoinSpaceStatusNeedsAttention{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterJoinSpaceAttentionReasonINSTANCE.Read(reader),
			FfiConverterJoinSpaceAttentionRecoveryINSTANCE.Read(reader),
			FfiConverterOptionalInt64INSTANCE.Read(reader),
		}
	case 5:
		return JoinSpaceStatusRejected{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterJoinSpaceRejectionReasonINSTANCE.Read(reader),
		}
	case 6:
		return JoinSpaceStatusTerminated{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterJoinSpaceTerminationReasonINSTANCE.Read(reader),
		}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterJoinSpaceStatus.Read()", id))
	}
}

func (FfiConverterJoinSpaceStatus) Write(writer io.Writer, value JoinSpaceStatus) {
	switch variant_value := value.(type) {
	case JoinSpaceStatusActive:
		writeInt32(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterJoinedSpaceINSTANCE.Write(writer, variant_value.JoinedSpace)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.PeerUpgradeRequired)
	case JoinSpaceStatusPending:
		writeInt32(writer, 2)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.TargetSpaceId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.SponsorDeviceId)
		FfiConverterOptionalStringINSTANCE.Write(writer, variant_value.SponsorIdentityFingerprint)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.CancelRequested)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.PeerUpgradeRequired)
	case JoinSpaceStatusProcessing:
		writeInt32(writer, 3)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.TargetSpaceId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.SponsorDeviceId)
		FfiConverterStringINSTANCE.Write(writer, variant_value.SponsorIdentityFingerprint)
		FfiConverterBoolINSTANCE.Write(writer, variant_value.PeerUpgradeRequired)
	case JoinSpaceStatusNeedsAttention:
		writeInt32(writer, 4)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterJoinSpaceAttentionReasonINSTANCE.Write(writer, variant_value.Reason)
		FfiConverterJoinSpaceAttentionRecoveryINSTANCE.Write(writer, variant_value.Recovery)
		FfiConverterOptionalInt64INSTANCE.Write(writer, variant_value.NextRetryAtMs)
	case JoinSpaceStatusRejected:
		writeInt32(writer, 5)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterJoinSpaceRejectionReasonINSTANCE.Write(writer, variant_value.Reason)
	case JoinSpaceStatusTerminated:
		writeInt32(writer, 6)
		FfiConverterStringINSTANCE.Write(writer, variant_value.JoinId)
		FfiConverterJoinSpaceTerminationReasonINSTANCE.Write(writer, variant_value.Reason)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterJoinSpaceStatus.Write", value))
	}
}

type FfiDestroyerJoinSpaceStatus struct{}

func (_ FfiDestroyerJoinSpaceStatus) Destroy(value JoinSpaceStatus) {
	value.Destroy()
}

type JoinSpaceTerminationReason uint

const (
	JoinSpaceTerminationReasonCancelled  JoinSpaceTerminationReason = 1
	JoinSpaceTerminationReasonExpired    JoinSpaceTerminationReason = 2
	JoinSpaceTerminationReasonSuperseded JoinSpaceTerminationReason = 3
)

type FfiConverterJoinSpaceTerminationReason struct{}

var FfiConverterJoinSpaceTerminationReasonINSTANCE = FfiConverterJoinSpaceTerminationReason{}

func (c FfiConverterJoinSpaceTerminationReason) Lift(rb RustBufferI) JoinSpaceTerminationReason {
	return LiftFromRustBuffer[JoinSpaceTerminationReason](c, rb)
}

func (c FfiConverterJoinSpaceTerminationReason) Lower(value JoinSpaceTerminationReason) C.RustBuffer {
	return LowerIntoRustBuffer[JoinSpaceTerminationReason](c, value)
}

func (c FfiConverterJoinSpaceTerminationReason) LowerExternal(value JoinSpaceTerminationReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[JoinSpaceTerminationReason](c, value))
}
func (FfiConverterJoinSpaceTerminationReason) Read(reader io.Reader) JoinSpaceTerminationReason {
	id := readInt32(reader)
	return JoinSpaceTerminationReason(id)
}

func (FfiConverterJoinSpaceTerminationReason) Write(writer io.Writer, value JoinSpaceTerminationReason) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerJoinSpaceTerminationReason struct{}

func (_ FfiDestroyerJoinSpaceTerminationReason) Destroy(value JoinSpaceTerminationReason) {
}

type MembershipRemovalDecision uint

const (
	MembershipRemovalDecisionAccept MembershipRemovalDecision = 1
	MembershipRemovalDecisionReject MembershipRemovalDecision = 2
)

type FfiConverterMembershipRemovalDecision struct{}

var FfiConverterMembershipRemovalDecisionINSTANCE = FfiConverterMembershipRemovalDecision{}

func (c FfiConverterMembershipRemovalDecision) Lift(rb RustBufferI) MembershipRemovalDecision {
	return LiftFromRustBuffer[MembershipRemovalDecision](c, rb)
}

func (c FfiConverterMembershipRemovalDecision) Lower(value MembershipRemovalDecision) C.RustBuffer {
	return LowerIntoRustBuffer[MembershipRemovalDecision](c, value)
}

func (c FfiConverterMembershipRemovalDecision) LowerExternal(value MembershipRemovalDecision) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[MembershipRemovalDecision](c, value))
}
func (FfiConverterMembershipRemovalDecision) Read(reader io.Reader) MembershipRemovalDecision {
	id := readInt32(reader)
	return MembershipRemovalDecision(id)
}

func (FfiConverterMembershipRemovalDecision) Write(writer io.Writer, value MembershipRemovalDecision) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerMembershipRemovalDecision struct{}

func (_ FfiDestroyerMembershipRemovalDecision) Destroy(value MembershipRemovalDecision) {
}

type RelayEntrySource uint

const (
	RelayEntrySourceBuiltIn RelayEntrySource = 1
	RelayEntrySourceCustom  RelayEntrySource = 2
)

type FfiConverterRelayEntrySource struct{}

var FfiConverterRelayEntrySourceINSTANCE = FfiConverterRelayEntrySource{}

func (c FfiConverterRelayEntrySource) Lift(rb RustBufferI) RelayEntrySource {
	return LiftFromRustBuffer[RelayEntrySource](c, rb)
}

func (c FfiConverterRelayEntrySource) Lower(value RelayEntrySource) C.RustBuffer {
	return LowerIntoRustBuffer[RelayEntrySource](c, value)
}

func (c FfiConverterRelayEntrySource) LowerExternal(value RelayEntrySource) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RelayEntrySource](c, value))
}
func (FfiConverterRelayEntrySource) Read(reader io.Reader) RelayEntrySource {
	id := readInt32(reader)
	return RelayEntrySource(id)
}

func (FfiConverterRelayEntrySource) Write(writer io.Writer, value RelayEntrySource) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerRelayEntrySource struct{}

func (_ FfiDestroyerRelayEntrySource) Destroy(value RelayEntrySource) {
}

// Relay 路由方式。优先级：`Disabled`（仅局域网）> `Custom`（替换内置列表）> `BuiltIn`。
type RelayRoutingMode uint

const (
	RelayRoutingModeBuiltIn  RelayRoutingMode = 1
	RelayRoutingModeCustom   RelayRoutingMode = 2
	RelayRoutingModeDisabled RelayRoutingMode = 3
)

type FfiConverterRelayRoutingMode struct{}

var FfiConverterRelayRoutingModeINSTANCE = FfiConverterRelayRoutingMode{}

func (c FfiConverterRelayRoutingMode) Lift(rb RustBufferI) RelayRoutingMode {
	return LiftFromRustBuffer[RelayRoutingMode](c, rb)
}

func (c FfiConverterRelayRoutingMode) Lower(value RelayRoutingMode) C.RustBuffer {
	return LowerIntoRustBuffer[RelayRoutingMode](c, value)
}

func (c FfiConverterRelayRoutingMode) LowerExternal(value RelayRoutingMode) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RelayRoutingMode](c, value))
}
func (FfiConverterRelayRoutingMode) Read(reader io.Reader) RelayRoutingMode {
	id := readInt32(reader)
	return RelayRoutingMode(id)
}

func (FfiConverterRelayRoutingMode) Write(writer io.Writer, value RelayRoutingMode) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerRelayRoutingMode struct{}

func (_ FfiDestroyerRelayRoutingMode) Destroy(value RelayRoutingMode) {
}

type ResendEntryOutcome interface {
	Destroy()
}
type ResendEntryOutcomeCompleted struct {
	Accepted  uint64
	Duplicate uint64
	Offline   uint64
	Errored   uint64
	Pending   uint64
}

func (e ResendEntryOutcomeCompleted) Destroy() {
	FfiDestroyerUint64{}.Destroy(e.Accepted)
	FfiDestroyerUint64{}.Destroy(e.Duplicate)
	FfiDestroyerUint64{}.Destroy(e.Offline)
	FfiDestroyerUint64{}.Destroy(e.Errored)
	FfiDestroyerUint64{}.Destroy(e.Pending)
}

type ResendEntryOutcomeSynchronizationDisabled struct {
}

func (e ResendEntryOutcomeSynchronizationDisabled) Destroy() {
}

type ResendEntryOutcomeEntryNotFound struct {
	EntryId string
}

func (e ResendEntryOutcomeEntryNotFound) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
}

type ResendEntryOutcomeEntryNotResendable struct {
	EntryId string
	Reason  EntryNotResendableReason
}

func (e ResendEntryOutcomeEntryNotResendable) Destroy() {
	FfiDestroyerString{}.Destroy(e.EntryId)
	FfiDestroyerEntryNotResendableReason{}.Destroy(e.Reason)
}

type ResendEntryOutcomeTargetNotTrusted struct {
	DeviceId string
}

func (e ResendEntryOutcomeTargetNotTrusted) Destroy() {
	FfiDestroyerString{}.Destroy(e.DeviceId)
}

type ResendEntryOutcomeNoEligibleTargets struct {
}

func (e ResendEntryOutcomeNoEligibleTargets) Destroy() {
}

type FfiConverterResendEntryOutcome struct{}

var FfiConverterResendEntryOutcomeINSTANCE = FfiConverterResendEntryOutcome{}

func (c FfiConverterResendEntryOutcome) Lift(rb RustBufferI) ResendEntryOutcome {
	return LiftFromRustBuffer[ResendEntryOutcome](c, rb)
}

func (c FfiConverterResendEntryOutcome) Lower(value ResendEntryOutcome) C.RustBuffer {
	return LowerIntoRustBuffer[ResendEntryOutcome](c, value)
}

func (c FfiConverterResendEntryOutcome) LowerExternal(value ResendEntryOutcome) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ResendEntryOutcome](c, value))
}
func (FfiConverterResendEntryOutcome) Read(reader io.Reader) ResendEntryOutcome {
	id := readInt32(reader)
	switch id {
	case 1:
		return ResendEntryOutcomeCompleted{
			FfiConverterUint64INSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
			FfiConverterUint64INSTANCE.Read(reader),
		}
	case 2:
		return ResendEntryOutcomeSynchronizationDisabled{}
	case 3:
		return ResendEntryOutcomeEntryNotFound{
			FfiConverterStringINSTANCE.Read(reader),
		}
	case 4:
		return ResendEntryOutcomeEntryNotResendable{
			FfiConverterStringINSTANCE.Read(reader),
			FfiConverterEntryNotResendableReasonINSTANCE.Read(reader),
		}
	case 5:
		return ResendEntryOutcomeTargetNotTrusted{
			FfiConverterStringINSTANCE.Read(reader),
		}
	case 6:
		return ResendEntryOutcomeNoEligibleTargets{}
	default:
		panic(fmt.Sprintf("invalid enum value %v in FfiConverterResendEntryOutcome.Read()", id))
	}
}

func (FfiConverterResendEntryOutcome) Write(writer io.Writer, value ResendEntryOutcome) {
	switch variant_value := value.(type) {
	case ResendEntryOutcomeCompleted:
		writeInt32(writer, 1)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Accepted)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Duplicate)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Offline)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Errored)
		FfiConverterUint64INSTANCE.Write(writer, variant_value.Pending)
	case ResendEntryOutcomeSynchronizationDisabled:
		writeInt32(writer, 2)
	case ResendEntryOutcomeEntryNotFound:
		writeInt32(writer, 3)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
	case ResendEntryOutcomeEntryNotResendable:
		writeInt32(writer, 4)
		FfiConverterStringINSTANCE.Write(writer, variant_value.EntryId)
		FfiConverterEntryNotResendableReasonINSTANCE.Write(writer, variant_value.Reason)
	case ResendEntryOutcomeTargetNotTrusted:
		writeInt32(writer, 5)
		FfiConverterStringINSTANCE.Write(writer, variant_value.DeviceId)
	case ResendEntryOutcomeNoEligibleTargets:
		writeInt32(writer, 6)
	default:
		_ = variant_value
		panic(fmt.Sprintf("invalid enum value `%v` in FfiConverterResendEntryOutcome.Write", value))
	}
}

type FfiDestroyerResendEntryOutcome struct{}

func (_ FfiDestroyerResendEntryOutcome) Destroy(value ResendEntryOutcome) {
	value.Destroy()
}

type WorkspaceConvergenceFailureCategory uint

const (
	WorkspaceConvergenceFailureCategorySpaceMismatch       WorkspaceConvergenceFailureCategory = 1
	WorkspaceConvergenceFailureCategoryContinuityGap       WorkspaceConvergenceFailureCategory = 2
	WorkspaceConvergenceFailureCategoryIdentityMismatch    WorkspaceConvergenceFailureCategory = 3
	WorkspaceConvergenceFailureCategoryDigestConflict      WorkspaceConvergenceFailureCategory = 4
	WorkspaceConvergenceFailureCategoryUnauthorized        WorkspaceConvergenceFailureCategory = 5
	WorkspaceConvergenceFailureCategoryVersionIncompatible WorkspaceConvergenceFailureCategory = 6
	WorkspaceConvergenceFailureCategoryNoEffectiveMembers  WorkspaceConvergenceFailureCategory = 7
	WorkspaceConvergenceFailureCategoryStorage             WorkspaceConvergenceFailureCategory = 8
)

type FfiConverterWorkspaceConvergenceFailureCategory struct{}

var FfiConverterWorkspaceConvergenceFailureCategoryINSTANCE = FfiConverterWorkspaceConvergenceFailureCategory{}

func (c FfiConverterWorkspaceConvergenceFailureCategory) Lift(rb RustBufferI) WorkspaceConvergenceFailureCategory {
	return LiftFromRustBuffer[WorkspaceConvergenceFailureCategory](c, rb)
}

func (c FfiConverterWorkspaceConvergenceFailureCategory) Lower(value WorkspaceConvergenceFailureCategory) C.RustBuffer {
	return LowerIntoRustBuffer[WorkspaceConvergenceFailureCategory](c, value)
}

func (c FfiConverterWorkspaceConvergenceFailureCategory) LowerExternal(value WorkspaceConvergenceFailureCategory) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[WorkspaceConvergenceFailureCategory](c, value))
}
func (FfiConverterWorkspaceConvergenceFailureCategory) Read(reader io.Reader) WorkspaceConvergenceFailureCategory {
	id := readInt32(reader)
	return WorkspaceConvergenceFailureCategory(id)
}

func (FfiConverterWorkspaceConvergenceFailureCategory) Write(writer io.Writer, value WorkspaceConvergenceFailureCategory) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerWorkspaceConvergenceFailureCategory struct{}

func (_ FfiDestroyerWorkspaceConvergenceFailureCategory) Destroy(value WorkspaceConvergenceFailureCategory) {
}

type WorkspaceConvergencePhase uint

const (
	WorkspaceConvergencePhaseLocallyApplied   WorkspaceConvergencePhase = 1
	WorkspaceConvergencePhaseConverging       WorkspaceConvergencePhase = 2
	WorkspaceConvergencePhaseComplete         WorkspaceConvergencePhase = 3
	WorkspaceConvergencePhaseRecoveryRequired WorkspaceConvergencePhase = 4
)

type FfiConverterWorkspaceConvergencePhase struct{}

var FfiConverterWorkspaceConvergencePhaseINSTANCE = FfiConverterWorkspaceConvergencePhase{}

func (c FfiConverterWorkspaceConvergencePhase) Lift(rb RustBufferI) WorkspaceConvergencePhase {
	return LiftFromRustBuffer[WorkspaceConvergencePhase](c, rb)
}

func (c FfiConverterWorkspaceConvergencePhase) Lower(value WorkspaceConvergencePhase) C.RustBuffer {
	return LowerIntoRustBuffer[WorkspaceConvergencePhase](c, value)
}

func (c FfiConverterWorkspaceConvergencePhase) LowerExternal(value WorkspaceConvergencePhase) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[WorkspaceConvergencePhase](c, value))
}
func (FfiConverterWorkspaceConvergencePhase) Read(reader io.Reader) WorkspaceConvergencePhase {
	id := readInt32(reader)
	return WorkspaceConvergencePhase(id)
}

func (FfiConverterWorkspaceConvergencePhase) Write(writer io.Writer, value WorkspaceConvergencePhase) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerWorkspaceConvergencePhase struct{}

func (_ FfiDestroyerWorkspaceConvergencePhase) Destroy(value WorkspaceConvergencePhase) {
}

type FfiConverterOptionalUint64 struct{}

var FfiConverterOptionalUint64INSTANCE = FfiConverterOptionalUint64{}

func (c FfiConverterOptionalUint64) Lift(rb RustBufferI) *uint64 {
	return LiftFromRustBuffer[*uint64](c, rb)
}

func (_ FfiConverterOptionalUint64) Read(reader io.Reader) *uint64 {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterUint64INSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalUint64) Lower(value *uint64) C.RustBuffer {
	return LowerIntoRustBuffer[*uint64](c, value)
}

func (c FfiConverterOptionalUint64) LowerExternal(value *uint64) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*uint64](c, value))
}

func (_ FfiConverterOptionalUint64) Write(writer io.Writer, value *uint64) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterUint64INSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalUint64 struct{}

func (_ FfiDestroyerOptionalUint64) Destroy(value *uint64) {
	if value != nil {
		FfiDestroyerUint64{}.Destroy(*value)
	}
}

type FfiConverterOptionalInt64 struct{}

var FfiConverterOptionalInt64INSTANCE = FfiConverterOptionalInt64{}

func (c FfiConverterOptionalInt64) Lift(rb RustBufferI) *int64 {
	return LiftFromRustBuffer[*int64](c, rb)
}

func (_ FfiConverterOptionalInt64) Read(reader io.Reader) *int64 {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterInt64INSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalInt64) Lower(value *int64) C.RustBuffer {
	return LowerIntoRustBuffer[*int64](c, value)
}

func (c FfiConverterOptionalInt64) LowerExternal(value *int64) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*int64](c, value))
}

func (_ FfiConverterOptionalInt64) Write(writer io.Writer, value *int64) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterInt64INSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalInt64 struct{}

func (_ FfiDestroyerOptionalInt64) Destroy(value *int64) {
	if value != nil {
		FfiDestroyerInt64{}.Destroy(*value)
	}
}

type FfiConverterOptionalString struct{}

var FfiConverterOptionalStringINSTANCE = FfiConverterOptionalString{}

func (c FfiConverterOptionalString) Lift(rb RustBufferI) *string {
	return LiftFromRustBuffer[*string](c, rb)
}

func (_ FfiConverterOptionalString) Read(reader io.Reader) *string {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterStringINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalString) Lower(value *string) C.RustBuffer {
	return LowerIntoRustBuffer[*string](c, value)
}

func (c FfiConverterOptionalString) LowerExternal(value *string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*string](c, value))
}

func (_ FfiConverterOptionalString) Write(writer io.Writer, value *string) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalString struct{}

func (_ FfiDestroyerOptionalString) Destroy(value *string) {
	if value != nil {
		FfiDestroyerString{}.Destroy(*value)
	}
}

type FfiConverterOptionalBytes struct{}

var FfiConverterOptionalBytesINSTANCE = FfiConverterOptionalBytes{}

func (c FfiConverterOptionalBytes) Lift(rb RustBufferI) *[]byte {
	return LiftFromRustBuffer[*[]byte](c, rb)
}

func (_ FfiConverterOptionalBytes) Read(reader io.Reader) *[]byte {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBytesINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBytes) Lower(value *[]byte) C.RustBuffer {
	return LowerIntoRustBuffer[*[]byte](c, value)
}

func (c FfiConverterOptionalBytes) LowerExternal(value *[]byte) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]byte](c, value))
}

func (_ FfiConverterOptionalBytes) Write(writer io.Writer, value *[]byte) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBytesINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBytes struct{}

func (_ FfiDestroyerOptionalBytes) Destroy(value *[]byte) {
	if value != nil {
		FfiDestroyerBytes{}.Destroy(*value)
	}
}

type FfiConverterOptionalActiveClipboard struct{}

var FfiConverterOptionalActiveClipboardINSTANCE = FfiConverterOptionalActiveClipboard{}

func (c FfiConverterOptionalActiveClipboard) Lift(rb RustBufferI) *ActiveClipboard {
	return LiftFromRustBuffer[*ActiveClipboard](c, rb)
}

func (_ FfiConverterOptionalActiveClipboard) Read(reader io.Reader) *ActiveClipboard {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterActiveClipboardINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalActiveClipboard) Lower(value *ActiveClipboard) C.RustBuffer {
	return LowerIntoRustBuffer[*ActiveClipboard](c, value)
}

func (c FfiConverterOptionalActiveClipboard) LowerExternal(value *ActiveClipboard) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*ActiveClipboard](c, value))
}

func (_ FfiConverterOptionalActiveClipboard) Write(writer io.Writer, value *ActiveClipboard) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterActiveClipboardINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalActiveClipboard struct{}

func (_ FfiDestroyerOptionalActiveClipboard) Destroy(value *ActiveClipboard) {
	if value != nil {
		FfiDestroyerActiveClipboard{}.Destroy(*value)
	}
}

type FfiConverterOptionalBindingCollectorConfig struct{}

var FfiConverterOptionalBindingCollectorConfigINSTANCE = FfiConverterOptionalBindingCollectorConfig{}

func (c FfiConverterOptionalBindingCollectorConfig) Lift(rb RustBufferI) *BindingCollectorConfig {
	return LiftFromRustBuffer[*BindingCollectorConfig](c, rb)
}

func (_ FfiConverterOptionalBindingCollectorConfig) Read(reader io.Reader) *BindingCollectorConfig {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBindingCollectorConfigINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBindingCollectorConfig) Lower(value *BindingCollectorConfig) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingCollectorConfig](c, value)
}

func (c FfiConverterOptionalBindingCollectorConfig) LowerExternal(value *BindingCollectorConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingCollectorConfig](c, value))
}

func (_ FfiConverterOptionalBindingCollectorConfig) Write(writer io.Writer, value *BindingCollectorConfig) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBindingCollectorConfigINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBindingCollectorConfig struct{}

func (_ FfiDestroyerOptionalBindingCollectorConfig) Destroy(value *BindingCollectorConfig) {
	if value != nil {
		FfiDestroyerBindingCollectorConfig{}.Destroy(*value)
	}
}

type FfiConverterOptionalBindingFailure struct{}

var FfiConverterOptionalBindingFailureINSTANCE = FfiConverterOptionalBindingFailure{}

func (c FfiConverterOptionalBindingFailure) Lift(rb RustBufferI) *BindingFailure {
	return LiftFromRustBuffer[*BindingFailure](c, rb)
}

func (_ FfiConverterOptionalBindingFailure) Read(reader io.Reader) *BindingFailure {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBindingFailureINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBindingFailure) Lower(value *BindingFailure) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingFailure](c, value)
}

func (c FfiConverterOptionalBindingFailure) LowerExternal(value *BindingFailure) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingFailure](c, value))
}

func (_ FfiConverterOptionalBindingFailure) Write(writer io.Writer, value *BindingFailure) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBindingFailureINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBindingFailure struct{}

func (_ FfiDestroyerOptionalBindingFailure) Destroy(value *BindingFailure) {
	if value != nil {
		FfiDestroyerBindingFailure{}.Destroy(*value)
	}
}

type FfiConverterOptionalSendReport struct{}

var FfiConverterOptionalSendReportINSTANCE = FfiConverterOptionalSendReport{}

func (c FfiConverterOptionalSendReport) Lift(rb RustBufferI) *SendReport {
	return LiftFromRustBuffer[*SendReport](c, rb)
}

func (_ FfiConverterOptionalSendReport) Read(reader io.Reader) *SendReport {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSendReportINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSendReport) Lower(value *SendReport) C.RustBuffer {
	return LowerIntoRustBuffer[*SendReport](c, value)
}

func (c FfiConverterOptionalSendReport) LowerExternal(value *SendReport) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*SendReport](c, value))
}

func (_ FfiConverterOptionalSendReport) Write(writer io.Writer, value *SendReport) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSendReportINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSendReport struct{}

func (_ FfiDestroyerOptionalSendReport) Destroy(value *SendReport) {
	if value != nil {
		FfiDestroyerSendReport{}.Destroy(*value)
	}
}

type FfiConverterOptionalSpaceInvitation struct{}

var FfiConverterOptionalSpaceInvitationINSTANCE = FfiConverterOptionalSpaceInvitation{}

func (c FfiConverterOptionalSpaceInvitation) Lift(rb RustBufferI) *SpaceInvitation {
	return LiftFromRustBuffer[*SpaceInvitation](c, rb)
}

func (_ FfiConverterOptionalSpaceInvitation) Read(reader io.Reader) *SpaceInvitation {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSpaceInvitationINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSpaceInvitation) Lower(value *SpaceInvitation) C.RustBuffer {
	return LowerIntoRustBuffer[*SpaceInvitation](c, value)
}

func (c FfiConverterOptionalSpaceInvitation) LowerExternal(value *SpaceInvitation) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*SpaceInvitation](c, value))
}

func (_ FfiConverterOptionalSpaceInvitation) Write(writer io.Writer, value *SpaceInvitation) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSpaceInvitationINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSpaceInvitation struct{}

func (_ FfiDestroyerOptionalSpaceInvitation) Destroy(value *SpaceInvitation) {
	if value != nil {
		FfiDestroyerSpaceInvitation{}.Destroy(*value)
	}
}

type FfiConverterOptionalBindingCaptureEndReason struct{}

var FfiConverterOptionalBindingCaptureEndReasonINSTANCE = FfiConverterOptionalBindingCaptureEndReason{}

func (c FfiConverterOptionalBindingCaptureEndReason) Lift(rb RustBufferI) *BindingCaptureEndReason {
	return LiftFromRustBuffer[*BindingCaptureEndReason](c, rb)
}

func (_ FfiConverterOptionalBindingCaptureEndReason) Read(reader io.Reader) *BindingCaptureEndReason {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBindingCaptureEndReasonINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBindingCaptureEndReason) Lower(value *BindingCaptureEndReason) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingCaptureEndReason](c, value)
}

func (c FfiConverterOptionalBindingCaptureEndReason) LowerExternal(value *BindingCaptureEndReason) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingCaptureEndReason](c, value))
}

func (_ FfiConverterOptionalBindingCaptureEndReason) Write(writer io.Writer, value *BindingCaptureEndReason) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBindingCaptureEndReasonINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBindingCaptureEndReason struct{}

func (_ FfiDestroyerOptionalBindingCaptureEndReason) Destroy(value *BindingCaptureEndReason) {
	if value != nil {
		FfiDestroyerBindingCaptureEndReason{}.Destroy(*value)
	}
}

type FfiConverterOptionalBindingEvent struct{}

var FfiConverterOptionalBindingEventINSTANCE = FfiConverterOptionalBindingEvent{}

func (c FfiConverterOptionalBindingEvent) Lift(rb RustBufferI) *BindingEvent {
	return LiftFromRustBuffer[*BindingEvent](c, rb)
}

func (_ FfiConverterOptionalBindingEvent) Read(reader io.Reader) *BindingEvent {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBindingEventINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBindingEvent) Lower(value *BindingEvent) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingEvent](c, value)
}

func (c FfiConverterOptionalBindingEvent) LowerExternal(value *BindingEvent) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingEvent](c, value))
}

func (_ FfiConverterOptionalBindingEvent) Write(writer io.Writer, value *BindingEvent) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBindingEventINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBindingEvent struct{}

func (_ FfiDestroyerOptionalBindingEvent) Destroy(value *BindingEvent) {
	if value != nil {
		FfiDestroyerBindingEvent{}.Destroy(*value)
	}
}

type FfiConverterOptionalBindingObservabilityRemoteSetupFailure struct{}

var FfiConverterOptionalBindingObservabilityRemoteSetupFailureINSTANCE = FfiConverterOptionalBindingObservabilityRemoteSetupFailure{}

func (c FfiConverterOptionalBindingObservabilityRemoteSetupFailure) Lift(rb RustBufferI) *BindingObservabilityRemoteSetupFailure {
	return LiftFromRustBuffer[*BindingObservabilityRemoteSetupFailure](c, rb)
}

func (_ FfiConverterOptionalBindingObservabilityRemoteSetupFailure) Read(reader io.Reader) *BindingObservabilityRemoteSetupFailure {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBindingObservabilityRemoteSetupFailureINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBindingObservabilityRemoteSetupFailure) Lower(value *BindingObservabilityRemoteSetupFailure) C.RustBuffer {
	return LowerIntoRustBuffer[*BindingObservabilityRemoteSetupFailure](c, value)
}

func (c FfiConverterOptionalBindingObservabilityRemoteSetupFailure) LowerExternal(value *BindingObservabilityRemoteSetupFailure) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*BindingObservabilityRemoteSetupFailure](c, value))
}

func (_ FfiConverterOptionalBindingObservabilityRemoteSetupFailure) Write(writer io.Writer, value *BindingObservabilityRemoteSetupFailure) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBindingObservabilityRemoteSetupFailureINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBindingObservabilityRemoteSetupFailure struct{}

func (_ FfiDestroyerOptionalBindingObservabilityRemoteSetupFailure) Destroy(value *BindingObservabilityRemoteSetupFailure) {
	if value != nil {
		FfiDestroyerBindingObservabilityRemoteSetupFailure{}.Destroy(*value)
	}
}

type FfiConverterOptionalCustomRelayMutationRejection struct{}

var FfiConverterOptionalCustomRelayMutationRejectionINSTANCE = FfiConverterOptionalCustomRelayMutationRejection{}

func (c FfiConverterOptionalCustomRelayMutationRejection) Lift(rb RustBufferI) *CustomRelayMutationRejection {
	return LiftFromRustBuffer[*CustomRelayMutationRejection](c, rb)
}

func (_ FfiConverterOptionalCustomRelayMutationRejection) Read(reader io.Reader) *CustomRelayMutationRejection {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterCustomRelayMutationRejectionINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalCustomRelayMutationRejection) Lower(value *CustomRelayMutationRejection) C.RustBuffer {
	return LowerIntoRustBuffer[*CustomRelayMutationRejection](c, value)
}

func (c FfiConverterOptionalCustomRelayMutationRejection) LowerExternal(value *CustomRelayMutationRejection) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*CustomRelayMutationRejection](c, value))
}

func (_ FfiConverterOptionalCustomRelayMutationRejection) Write(writer io.Writer, value *CustomRelayMutationRejection) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterCustomRelayMutationRejectionINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalCustomRelayMutationRejection struct{}

func (_ FfiDestroyerOptionalCustomRelayMutationRejection) Destroy(value *CustomRelayMutationRejection) {
	if value != nil {
		FfiDestroyerCustomRelayMutationRejection{}.Destroy(*value)
	}
}

type FfiConverterOptionalRelayRoutingMode struct{}

var FfiConverterOptionalRelayRoutingModeINSTANCE = FfiConverterOptionalRelayRoutingMode{}

func (c FfiConverterOptionalRelayRoutingMode) Lift(rb RustBufferI) *RelayRoutingMode {
	return LiftFromRustBuffer[*RelayRoutingMode](c, rb)
}

func (_ FfiConverterOptionalRelayRoutingMode) Read(reader io.Reader) *RelayRoutingMode {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterRelayRoutingModeINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalRelayRoutingMode) Lower(value *RelayRoutingMode) C.RustBuffer {
	return LowerIntoRustBuffer[*RelayRoutingMode](c, value)
}

func (c FfiConverterOptionalRelayRoutingMode) LowerExternal(value *RelayRoutingMode) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*RelayRoutingMode](c, value))
}

func (_ FfiConverterOptionalRelayRoutingMode) Write(writer io.Writer, value *RelayRoutingMode) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterRelayRoutingModeINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalRelayRoutingMode struct{}

func (_ FfiDestroyerOptionalRelayRoutingMode) Destroy(value *RelayRoutingMode) {
	if value != nil {
		FfiDestroyerRelayRoutingMode{}.Destroy(*value)
	}
}

type FfiConverterOptionalWorkspaceConvergenceFailureCategory struct{}

var FfiConverterOptionalWorkspaceConvergenceFailureCategoryINSTANCE = FfiConverterOptionalWorkspaceConvergenceFailureCategory{}

func (c FfiConverterOptionalWorkspaceConvergenceFailureCategory) Lift(rb RustBufferI) *WorkspaceConvergenceFailureCategory {
	return LiftFromRustBuffer[*WorkspaceConvergenceFailureCategory](c, rb)
}

func (_ FfiConverterOptionalWorkspaceConvergenceFailureCategory) Read(reader io.Reader) *WorkspaceConvergenceFailureCategory {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterWorkspaceConvergenceFailureCategoryINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalWorkspaceConvergenceFailureCategory) Lower(value *WorkspaceConvergenceFailureCategory) C.RustBuffer {
	return LowerIntoRustBuffer[*WorkspaceConvergenceFailureCategory](c, value)
}

func (c FfiConverterOptionalWorkspaceConvergenceFailureCategory) LowerExternal(value *WorkspaceConvergenceFailureCategory) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*WorkspaceConvergenceFailureCategory](c, value))
}

func (_ FfiConverterOptionalWorkspaceConvergenceFailureCategory) Write(writer io.Writer, value *WorkspaceConvergenceFailureCategory) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterWorkspaceConvergenceFailureCategoryINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalWorkspaceConvergenceFailureCategory struct{}

func (_ FfiDestroyerOptionalWorkspaceConvergenceFailureCategory) Destroy(value *WorkspaceConvergenceFailureCategory) {
	if value != nil {
		FfiDestroyerWorkspaceConvergenceFailureCategory{}.Destroy(*value)
	}
}

type FfiConverterSequenceString struct{}

var FfiConverterSequenceStringINSTANCE = FfiConverterSequenceString{}

func (c FfiConverterSequenceString) Lift(rb RustBufferI) []string {
	return LiftFromRustBuffer[[]string](c, rb)
}

func (c FfiConverterSequenceString) Read(reader io.Reader) []string {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]string, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterStringINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceString) Lower(value []string) C.RustBuffer {
	return LowerIntoRustBuffer[[]string](c, value)
}

func (c FfiConverterSequenceString) LowerExternal(value []string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]string](c, value))
}

func (c FfiConverterSequenceString) Write(writer io.Writer, value []string) {
	if len(value) > math.MaxInt32 {
		panic("[]string is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterStringINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceString struct{}

func (FfiDestroyerSequenceString) Destroy(sequence []string) {
	for _, value := range sequence {
		FfiDestroyerString{}.Destroy(value)
	}
}

type FfiConverterSequenceBindingFileSourceCounts struct{}

var FfiConverterSequenceBindingFileSourceCountsINSTANCE = FfiConverterSequenceBindingFileSourceCounts{}

func (c FfiConverterSequenceBindingFileSourceCounts) Lift(rb RustBufferI) []BindingFileSourceCounts {
	return LiftFromRustBuffer[[]BindingFileSourceCounts](c, rb)
}

func (c FfiConverterSequenceBindingFileSourceCounts) Read(reader io.Reader) []BindingFileSourceCounts {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]BindingFileSourceCounts, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterBindingFileSourceCountsINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceBindingFileSourceCounts) Lower(value []BindingFileSourceCounts) C.RustBuffer {
	return LowerIntoRustBuffer[[]BindingFileSourceCounts](c, value)
}

func (c FfiConverterSequenceBindingFileSourceCounts) LowerExternal(value []BindingFileSourceCounts) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]BindingFileSourceCounts](c, value))
}

func (c FfiConverterSequenceBindingFileSourceCounts) Write(writer io.Writer, value []BindingFileSourceCounts) {
	if len(value) > math.MaxInt32 {
		panic("[]BindingFileSourceCounts is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterBindingFileSourceCountsINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceBindingFileSourceCounts struct{}

func (FfiDestroyerSequenceBindingFileSourceCounts) Destroy(sequence []BindingFileSourceCounts) {
	for _, value := range sequence {
		FfiDestroyerBindingFileSourceCounts{}.Destroy(value)
	}
}

type FfiConverterSequenceBindingSourceCoverage struct{}

var FfiConverterSequenceBindingSourceCoverageINSTANCE = FfiConverterSequenceBindingSourceCoverage{}

func (c FfiConverterSequenceBindingSourceCoverage) Lift(rb RustBufferI) []BindingSourceCoverage {
	return LiftFromRustBuffer[[]BindingSourceCoverage](c, rb)
}

func (c FfiConverterSequenceBindingSourceCoverage) Read(reader io.Reader) []BindingSourceCoverage {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]BindingSourceCoverage, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterBindingSourceCoverageINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceBindingSourceCoverage) Lower(value []BindingSourceCoverage) C.RustBuffer {
	return LowerIntoRustBuffer[[]BindingSourceCoverage](c, value)
}

func (c FfiConverterSequenceBindingSourceCoverage) LowerExternal(value []BindingSourceCoverage) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]BindingSourceCoverage](c, value))
}

func (c FfiConverterSequenceBindingSourceCoverage) Write(writer io.Writer, value []BindingSourceCoverage) {
	if len(value) > math.MaxInt32 {
		panic("[]BindingSourceCoverage is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterBindingSourceCoverageINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceBindingSourceCoverage struct{}

func (FfiDestroyerSequenceBindingSourceCoverage) Destroy(sequence []BindingSourceCoverage) {
	for _, value := range sequence {
		FfiDestroyerBindingSourceCoverage{}.Destroy(value)
	}
}

type FfiConverterSequenceCustomRelay struct{}

var FfiConverterSequenceCustomRelayINSTANCE = FfiConverterSequenceCustomRelay{}

func (c FfiConverterSequenceCustomRelay) Lift(rb RustBufferI) []CustomRelay {
	return LiftFromRustBuffer[[]CustomRelay](c, rb)
}

func (c FfiConverterSequenceCustomRelay) Read(reader io.Reader) []CustomRelay {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]CustomRelay, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterCustomRelayINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceCustomRelay) Lower(value []CustomRelay) C.RustBuffer {
	return LowerIntoRustBuffer[[]CustomRelay](c, value)
}

func (c FfiConverterSequenceCustomRelay) LowerExternal(value []CustomRelay) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]CustomRelay](c, value))
}

func (c FfiConverterSequenceCustomRelay) Write(writer io.Writer, value []CustomRelay) {
	if len(value) > math.MaxInt32 {
		panic("[]CustomRelay is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterCustomRelayINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceCustomRelay struct{}

func (FfiDestroyerSequenceCustomRelay) Destroy(sequence []CustomRelay) {
	for _, value := range sequence {
		FfiDestroyerCustomRelay{}.Destroy(value)
	}
}

type FfiConverterSequenceDevice struct{}

var FfiConverterSequenceDeviceINSTANCE = FfiConverterSequenceDevice{}

func (c FfiConverterSequenceDevice) Lift(rb RustBufferI) []Device {
	return LiftFromRustBuffer[[]Device](c, rb)
}

func (c FfiConverterSequenceDevice) Read(reader io.Reader) []Device {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]Device, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterDeviceINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceDevice) Lower(value []Device) C.RustBuffer {
	return LowerIntoRustBuffer[[]Device](c, value)
}

func (c FfiConverterSequenceDevice) LowerExternal(value []Device) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]Device](c, value))
}

func (c FfiConverterSequenceDevice) Write(writer io.Writer, value []Device) {
	if len(value) > math.MaxInt32 {
		panic("[]Device is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterDeviceINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceDevice struct{}

func (FfiDestroyerSequenceDevice) Destroy(sequence []Device) {
	for _, value := range sequence {
		FfiDestroyerDevice{}.Destroy(value)
	}
}

type FfiConverterSequenceRelayOverviewEntry struct{}

var FfiConverterSequenceRelayOverviewEntryINSTANCE = FfiConverterSequenceRelayOverviewEntry{}

func (c FfiConverterSequenceRelayOverviewEntry) Lift(rb RustBufferI) []RelayOverviewEntry {
	return LiftFromRustBuffer[[]RelayOverviewEntry](c, rb)
}

func (c FfiConverterSequenceRelayOverviewEntry) Read(reader io.Reader) []RelayOverviewEntry {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]RelayOverviewEntry, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterRelayOverviewEntryINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceRelayOverviewEntry) Lower(value []RelayOverviewEntry) C.RustBuffer {
	return LowerIntoRustBuffer[[]RelayOverviewEntry](c, value)
}

func (c FfiConverterSequenceRelayOverviewEntry) LowerExternal(value []RelayOverviewEntry) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]RelayOverviewEntry](c, value))
}

func (c FfiConverterSequenceRelayOverviewEntry) Write(writer io.Writer, value []RelayOverviewEntry) {
	if len(value) > math.MaxInt32 {
		panic("[]RelayOverviewEntry is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterRelayOverviewEntryINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceRelayOverviewEntry struct{}

func (FfiDestroyerSequenceRelayOverviewEntry) Destroy(sequence []RelayOverviewEntry) {
	for _, value := range sequence {
		FfiDestroyerRelayOverviewEntry{}.Destroy(value)
	}
}

type FfiConverterSequenceBindingClipboardRepresentation struct{}

var FfiConverterSequenceBindingClipboardRepresentationINSTANCE = FfiConverterSequenceBindingClipboardRepresentation{}

func (c FfiConverterSequenceBindingClipboardRepresentation) Lift(rb RustBufferI) []BindingClipboardRepresentation {
	return LiftFromRustBuffer[[]BindingClipboardRepresentation](c, rb)
}

func (c FfiConverterSequenceBindingClipboardRepresentation) Read(reader io.Reader) []BindingClipboardRepresentation {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]BindingClipboardRepresentation, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterBindingClipboardRepresentationINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceBindingClipboardRepresentation) Lower(value []BindingClipboardRepresentation) C.RustBuffer {
	return LowerIntoRustBuffer[[]BindingClipboardRepresentation](c, value)
}

func (c FfiConverterSequenceBindingClipboardRepresentation) LowerExternal(value []BindingClipboardRepresentation) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]BindingClipboardRepresentation](c, value))
}

func (c FfiConverterSequenceBindingClipboardRepresentation) Write(writer io.Writer, value []BindingClipboardRepresentation) {
	if len(value) > math.MaxInt32 {
		panic("[]BindingClipboardRepresentation is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterBindingClipboardRepresentationINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceBindingClipboardRepresentation struct{}

func (FfiDestroyerSequenceBindingClipboardRepresentation) Destroy(sequence []BindingClipboardRepresentation) {
	for _, value := range sequence {
		FfiDestroyerBindingClipboardRepresentation{}.Destroy(value)
	}
}

func CoreVersion() string {
	return FfiConverterStringINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_core_version(_uniffiStatus),
		}
	}))
}

func FlushProcessObservability(deadlineMs uint64) (BindingObservabilityFlushSummary, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_flush_process_observability(FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingObservabilityFlushSummary
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingObservabilityFlushSummaryINSTANCE.Lift(_uniffiRV), nil
	}
}

func InstallProcessObservability(config BindingObservabilityConfig, host BindingHost) (BindingObservabilitySetup, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_install_process_observability(FfiConverterBindingObservabilityConfigINSTANCE.Lower(config), FfiConverterBindingHostINSTANCE.Lower(host), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingObservabilitySetup
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingObservabilitySetupINSTANCE.Lift(_uniffiRV), nil
	}
}

func QueryProcessObservabilityHealth() (BindingObservabilityHealth, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_query_process_observability_health(_uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingObservabilityHealth
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingObservabilityHealthINSTANCE.Lift(_uniffiRV), nil
	}
}

func ShutdownProcessObservability(deadlineMs uint64) (BindingObservabilityShutdownSummary, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingError](FfiConverterBindingError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_shutdown_process_observability(FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingObservabilityShutdownSummary
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingObservabilityShutdownSummaryINSTANCE.Lift(_uniffiRV), nil
	}
}

// 与原有 flush 一样，宿主须在原生后台工作队列调用，不能在 UI 主线程等待。
func PrepareLocalDiagnosticExport(deadlineMs uint64) (BindingLocalDiagnosticExportReport, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_prepare_local_diagnostic_export(FfiConverterUint64INSTANCE.Lower(deadlineMs), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingLocalDiagnosticExportReport
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingLocalDiagnosticExportReportINSTANCE.Lift(_uniffiRV), nil
	}
}

func QueryLocalDiagnosticStatus() (BindingLocalDiagnosticStatus, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_query_local_diagnostic_status(_uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingLocalDiagnosticStatus
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingLocalDiagnosticStatusINSTANCE.Lift(_uniffiRV), nil
	}
}

func RecordHostDiagnostic(source BindingHostDiagnosticSource, event BindingHostDiagnosticEvent) (BindingHostDiagnosticReceipt, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_record_host_diagnostic(FfiConverterBindingHostDiagnosticSourceINSTANCE.Lower(source), FfiConverterBindingHostDiagnosticEventINSTANCE.Lower(event), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingHostDiagnosticReceipt
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingHostDiagnosticReceiptINSTANCE.Lift(_uniffiRV), nil
	}
}

func RegisterHostDiagnosticSource(source BindingHostDiagnosticSource, capability BindingSourceCapability) error {
	_, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) bool {
		C.uniffi_uc_engine_uniffi_fn_func_register_host_diagnostic_source(FfiConverterBindingHostDiagnosticSourceINSTANCE.Lower(source), FfiConverterBindingSourceCapabilityINSTANCE.Lower(capability), _uniffiStatus)
		return false
	})
	return _uniffiErr.AsError()
}

func StartLocalDiagnosticCapture(durationMs uint64) (BindingLocalCaptureStatus, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_start_local_diagnostic_capture(FfiConverterUint64INSTANCE.Lower(durationMs), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingLocalCaptureStatus
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingLocalCaptureStatusINSTANCE.Lift(_uniffiRV), nil
	}
}

func StopLocalDiagnosticCapture(captureId string) (BindingStopCaptureResult, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*BindingLocalDiagnosticError](FfiConverterBindingLocalDiagnosticError{}, func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_uc_engine_uniffi_fn_func_stop_local_diagnostic_capture(FfiConverterStringINSTANCE.Lower(captureId), _uniffiStatus),
		}
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue BindingStopCaptureResult
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterBindingStopCaptureResultINSTANCE.Lift(_uniffiRV), nil
	}
}
