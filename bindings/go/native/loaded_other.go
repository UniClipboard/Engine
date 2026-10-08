//go:build !(cgo && (darwin || linux))

package native

// 其他平台尚无已验证的加载路径解析；明确失败而不是静默通过。
func loadedLibraryPath() (string, error) {
	return "", ErrUnsupported
}
