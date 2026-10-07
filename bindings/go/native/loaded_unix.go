//go:build cgo && (darwin || linux)

package native

/*
#cgo LDFLAGS: -luc_engine_uniffi
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdint.h>

// 经 dlsym 取符号：直接取导入函数的地址在 Linux 上可能得到可执行文件内的 PLT 桩，
// dladdr 会返回可执行文件而不是共享库。
static const char *uc_loaded_library_path(void) {
	void *symbol = dlsym(RTLD_DEFAULT, "ffi_uc_engine_uniffi_uniffi_contract_version");
	Dl_info info;
	if (symbol == 0 || dladdr(symbol, &info) == 0) {
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
