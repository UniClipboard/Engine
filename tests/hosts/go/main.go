// 命令 go-engine-host 是 Go 绑定的真实进程验收宿主，不承载产品功能。
//
//	go-engine-host --phase lifecycle|restart|negative|verify --root <隔离目录> --manifest <native-manifest.json>
//
// 每个阶段是一次独立进程：向 stdout 输出一个 JSON 结果，任一步失败则退出码为 1。
// 隔离目录之外不读写任何文件；网络隔离由外部驱动脚本（沙箱）保证。
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
)

const (
	appVersion = "1.1.0-rc.22"
	profileID  = "go-binding-sentinel-profile"
)

type step struct {
	Name   string `json:"name"`
	OK     bool   `json:"ok"`
	Detail any    `json:"detail,omitempty"`
}

type report struct {
	Phase string `json:"phase"`
	Steps []step `json:"steps"`
	OK    bool   `json:"ok"`
}

func (r *report) record(name string, ok bool, detail any) bool {
	r.Steps = append(r.Steps, step{Name: name, OK: ok, Detail: detail})
	return ok
}

func main() {
	phase := flag.String("phase", "", "lifecycle, restart, negative or verify")
	root := flag.String("root", "", "isolated root directory")
	manifest := flag.String("manifest", "", "native-manifest.json path")
	flag.Parse()
	if *phase == "" || *root == "" || *manifest == "" {
		fmt.Fprintln(os.Stderr, "usage: go-engine-host --phase P --root DIR --manifest FILE")
		os.Exit(2)
	}
	result := &report{Phase: *phase}
	switch *phase {
	case "lifecycle":
		runLifecycle(result, *root, *manifest)
	case "restart":
		runRestart(result, *root, *manifest)
	case "negative":
		runNegative(result, *root, *manifest)
	case "verify":
		runVerify(result, *manifest)
	default:
		fmt.Fprintln(os.Stderr, "unknown phase")
		os.Exit(2)
	}
	result.OK = true
	for _, s := range result.Steps {
		result.OK = result.OK && s.OK
	}
	encoded, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		os.Exit(2)
	}
	fmt.Println(string(encoded))
	if !result.OK {
		os.Exit(1)
	}
}
