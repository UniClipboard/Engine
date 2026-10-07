//go:build cgo && (darwin || linux)

package native

/*
#cgo LDFLAGS: -luc_engine_uniffi
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdint.h>

extern uint32_t ffi_uc_engine_uniffi_uniffi_contract_version(void);

static const char *uc_loaded_library_path(void) {
	Dl_info info;
	if (dladdr((void *)ffi_uc_engine_uniffi_uniffi_contract_version, &info) == 0) {
		return 0;
	}
	return info.dli_fname;
}
*/
import "C"

import (
	"fmt"
	"path/filepath"
)

// loadedLibraryPath 通过原生库内的符号向动态链接器查询实际映射的文件，而不是信任 rpath 配置。
func loadedLibraryPath() (string, error) {
	name := C.uc_loaded_library_path()
	if name == nil {
		return "", fmt.Errorf("%w: loader did not report the library path", ErrMismatch)
	}
	return filepath.Clean(C.GoString(name)), nil
}
