// Package native 在启动 Engine 之前核对实际加载的原生库与来源清单一致。
//
// 清单由 stage-native.sh 与原生库一同生成。Verify 不提供“跳过校验”的模式：
// 清单缺失、字段不符、库被替换或版本错配都返回 ErrMismatch。
package native

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"

	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

const manifestSchema = 1

var (
	// ErrMismatch 表示清单缺失、无法解析，或其内容与实际加载的原生库不一致。
	ErrMismatch = errors.New("native library does not match its provenance manifest")
	// ErrUnsupported 表示当前平台没有加载路径解析，无法完成校验；调用方不得当作通过。
	ErrUnsupported = errors.New("native library identity check is not supported on this platform")
)

// Manifest 是 native-manifest.json 的内容。
type Manifest struct {
	Schema          int      `json:"schema"`
	EngineRevision  string   `json:"engine_revision"`
	EngineVersion   string   `json:"engine_version"`
	CargoLockSHA256 string   `json:"cargo_lock_sha256"`
	UniffiVersion   string   `json:"uniffi_version"`
	Target          string   `json:"target"`
	Profile         string   `json:"profile"`
	Features        []string `json:"features"`
	Rustc           string   `json:"rustc"`
	Cargo           string   `json:"cargo"`
	Generator       struct {
		Revision    string `json:"revision"`
		Tag         string `json:"tag"`
		LockSHA256  string `json:"lock_sha256"`
		PatchSHA256 string `json:"patch_sha256"`
	} `json:"generator"`
	GeneratedSourcesSHA256 string `json:"generated_sources_sha256"`
	Library                struct {
		File                string `json:"file"`
		SHA256              string `json:"sha256"`
		Size                int64  `json:"size"`
		CargoArtifactSHA256 string `json:"cargo_artifact_sha256"`
	} `json:"library"`
}

var (
	verifyMu sync.Mutex
	verified = map[string]*Manifest{}
)

// Verify 读取清单并核对实际加载的原生库；同一路径在进程内只做一次完整校验。
//
// 这是完整性与一致性自检：能发现版本错配、陈旧库、目标不符和与已编译绑定不一致的清单，
// 但清单与库通常在同一目录，它不是认证，也不防御有本地写权限的攻击者。校验对象是动态链接器
// 报告的文件路径上的内容；静态链接（库不是独立文件）时库名比较失败，按错配拒绝。
func Verify(manifestPath string) (*Manifest, error) {
	absolute, err := filepath.Abs(manifestPath)
	if err != nil {
		return nil, fmt.Errorf("%w: manifest path", ErrMismatch)
	}
	verifyMu.Lock()
	defer verifyMu.Unlock()
	if manifest, ok := verified[absolute]; ok {
		return manifest, nil
	}
	manifest, err := readManifest(absolute)
	if err != nil {
		return nil, err
	}
	if err := checkLoadedLibrary(manifest); err != nil {
		return nil, err
	}
	verified[absolute] = manifest
	return manifest, nil
}

func readManifest(path string) (*Manifest, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("%w: manifest unreadable", ErrMismatch)
	}
	var manifest Manifest
	if err := json.Unmarshal(data, &manifest); err != nil {
		return nil, fmt.Errorf("%w: manifest malformed", ErrMismatch)
	}
	if manifest.Schema != manifestSchema {
		return nil, fmt.Errorf("%w: manifest schema %d", ErrMismatch, manifest.Schema)
	}
	for field, value := range map[string]string{
		"engine_revision":          manifest.EngineRevision,
		"engine_version":           manifest.EngineVersion,
		"cargo_lock_sha256":        manifest.CargoLockSHA256,
		"target":                   manifest.Target,
		"profile":                  manifest.Profile,
		"generator.revision":       manifest.Generator.Revision,
		"generator.patch_sha256":   manifest.Generator.PatchSHA256,
		"generated_sources_sha256": manifest.GeneratedSourcesSHA256,
		"library.file":             manifest.Library.File,
		"library.sha256":           manifest.Library.SHA256,
	} {
		if value == "" {
			return nil, fmt.Errorf("%w: manifest field %s is empty", ErrMismatch, field)
		}
	}
	return &manifest, nil
}

// targetMatches 判断清单的 Rust 目标三元组是否对应当前 Go 进程的 GOOS/GOARCH。
func targetMatches(target string) bool {
	arch := map[string]string{"arm64": "aarch64", "amd64": "x86_64"}[runtime.GOARCH]
	osName := map[string]string{"darwin": "apple-darwin", "linux": "linux", "windows": "windows"}[runtime.GOOS]
	return arch != "" && osName != "" &&
		strings.HasPrefix(target, arch+"-") && strings.Contains(target, osName)
}

func checkLoadedLibrary(manifest *Manifest) error {
	if !targetMatches(manifest.Target) {
		return fmt.Errorf("%w: manifest target does not match this process", ErrMismatch)
	}
	if manifest.GeneratedSourcesSHA256 != ffi.GeneratedSourcesSHA256 {
		return fmt.Errorf("%w: manifest was produced for different generated bindings", ErrMismatch)
	}
	if got, want := ffi.CoreVersion(), "v"+manifest.EngineVersion; got != want {
		return fmt.Errorf("%w: core version %s, manifest %s", ErrMismatch, got, want)
	}
	loaded, err := loadedLibraryPath()
	if err != nil {
		return err
	}
	if filepath.Base(loaded) != manifest.Library.File {
		return fmt.Errorf("%w: loaded library name differs from manifest", ErrMismatch)
	}
	sum, size, err := hashFile(loaded)
	if err != nil {
		return fmt.Errorf("%w: loaded library unreadable", ErrMismatch)
	}
	if size != manifest.Library.Size || sum != manifest.Library.SHA256 {
		return fmt.Errorf("%w: loaded library digest differs from manifest", ErrMismatch)
	}
	return nil
}

func hashFile(path string) (string, int64, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", 0, err
	}
	defer file.Close()
	digest := sha256.New()
	size, err := io.Copy(digest, file)
	if err != nil {
		return "", 0, err
	}
	return hex.EncodeToString(digest.Sum(nil)), size, nil
}
