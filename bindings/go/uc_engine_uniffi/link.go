// 手写文件：cgo 编译与链接指令。生成文件由 generator/generate.sh 重写，本文件不受影响。
//
// 头文件随模块分发；原生库不在 Go module 内，由宿主构建环境经 CGO_LDFLAGS 提供 -L 与 rpath，
// 见 ../README.md。动态库身份由 ../native 的清单校验。

package uc_engine_uniffi

// #cgo CFLAGS: -I${SRCDIR}
// #cgo LDFLAGS: -luc_engine_uniffi
import "C"
