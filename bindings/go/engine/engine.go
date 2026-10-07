// Package engine 是 UniClipboard Engine 的 Go 门面：一次 Open、查询、事件、确定性 Close。
//
// 所有权与并发契约：
//   - Open 先核对原生库来源清单（native.Verify），失败即拒绝，没有跳过校验的模式。
//   - 所有方法可并发调用；Rust 的命令队列无界，限流由调用方负责。
//   - Close 先拒绝新调用，再请求 Rust 在期限内关闭并 join，然后等待在途调用排空，最后释放生成层对象。
//     Close 之后的调用返回 ErrClosed，不会 panic。Close 可重复调用。
//   - context 取消只让调用方停止等待：Rust 调用继续执行，Close 仍会等待它结束。
//   - 事件队列容量有限，溢出时 Rust 丢弃最旧事件并给出 RefreshRequired(ConsumerLagged)；
//     门面不再设第二层缓冲，消费者应在收到后重新查询权威状态。
//   - Rust 在发布构建中 panic=abort：Rust 崩溃会直接终止整个宿主进程，Go 的 recover 无效。
package engine

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"time"

	"github.com/UniClipboard/Engine/bindings/go/native"
	ffi "github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi"
)

// eventPollSlice 是 NextEvent 每次进入 Rust 等待的时长，使 context 取消和 Close 能及时生效。
const eventPollSlice = 200 * time.Millisecond

// Config 是 Open 的输入。
type Config struct {
	AppVersion string
	ProfileID  string
	// NativeManifest 是与原生库一同交付的 native-manifest.json 路径，必填。
	NativeManifest string
}

// LocalDevice 是本机设备身份。
type LocalDevice struct {
	DeviceID    string
	DisplayName string
}

// SpaceState 是当前空间状态的摘要；不含邀请内容。
type SpaceState struct {
	HasCompleted      bool
	RePairingRequired bool
	SpaceID           string
	HasInvitation     bool
	DeviceName        string
}

// Device 是设备组中的一台设备。
type Device struct {
	DeviceID    string
	DisplayName string
	IsLocal     bool
	Online      bool
}

// Opportunity 是触发网络恢复的外部时机。零值非法。
type Opportunity int

const (
	OpportunityForeground Opportunity = iota + 1
	OpportunitySystemWake
	OpportunityNetworkChanged
)

// Engine 是一个运行中的 Engine 实例。
type Engine struct {
	inner *ffi.MobileEngine

	closeMu  sync.Mutex // 串行化 Close，避免第二个 Close 对已释放的对象调用 Shutdown
	mu       sync.Mutex
	inflight int
	closing  bool
	idle     chan struct{} // 关闭后在途调用排空时关闭
	closed   bool          // 生成层对象已释放
}

// Open 核对原生库并启动 Engine。host 不得为空，回调规则见 Host。
func Open(config Config, host Host) (*Engine, error) {
	if config.AppVersion == "" || config.ProfileID == "" || config.NativeManifest == "" || host == nil {
		return nil, ErrInvalidInput
	}
	if _, err := native.Verify(config.NativeManifest); err != nil {
		return nil, err
	}
	inner, err := ffi.MobileEngineStart(
		ffi.BindingConfig{AppVersion: config.AppVersion, ProfileId: config.ProfileID},
		hostAdapter{host: host},
	)
	if err != nil {
		return nil, convertError(err)
	}
	return &Engine{inner: inner, idle: make(chan struct{})}, nil
}

func (e *Engine) begin() error {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.closing {
		return ErrClosed
	}
	e.inflight++
	return nil
}

func (e *Engine) end() {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.inflight--
	if e.closing && e.inflight == 0 {
		close(e.idle)
	}
}

// call 在 Rust 调用期间登记在途状态。ctx 先结束时返回 ctx.Err()，调用本身继续执行到完成。
func call[T any](ctx context.Context, e *Engine, fn func(*ffi.MobileEngine) (T, error)) (T, error) {
	var zero T
	if err := ctx.Err(); err != nil {
		return zero, err
	}
	if err := e.begin(); err != nil {
		return zero, err
	}
	type outcome struct {
		value T
		err   error
	}
	done := make(chan outcome, 1)
	go func() {
		defer e.end()
		// 生成层的 panic（例如对象已释放）不能终止宿主进程；Rust 侧的 abort 不在此列。
		defer func() {
			if recover() != nil {
				done <- outcome{err: ErrUnexpectedResult}
			}
		}()
		value, err := fn(e.inner)
		done <- outcome{value, convertError(err)}
	}()
	select {
	case result := <-done:
		return result.value, result.err
	case <-ctx.Done():
		return zero, ctx.Err()
	}
}

// LocalDevice 查询本机设备身份。
func (e *Engine) LocalDevice(ctx context.Context) (LocalDevice, error) {
	return call(ctx, e, func(inner *ffi.MobileEngine) (LocalDevice, error) {
		device, err := inner.QueryLocalDevice()
		return LocalDevice{DeviceID: device.DeviceId, DisplayName: device.DisplayName}, err
	})
}

// SpaceState 查询当前空间状态。
func (e *Engine) SpaceState(ctx context.Context) (SpaceState, error) {
	return call(ctx, e, func(inner *ffi.MobileEngine) (SpaceState, error) {
		state, err := inner.QuerySpaceState()
		result := SpaceState{
			HasCompleted:      state.HasCompleted,
			RePairingRequired: state.RePairingRequired,
			HasInvitation:     state.CurrentInvitation != nil,
		}
		if state.SpaceId != nil {
			result.SpaceID = *state.SpaceId
		}
		if state.DeviceName != nil {
			result.DeviceName = *state.DeviceName
		}
		return result, err
	})
}

// Devices 列出设备组。
func (e *Engine) Devices(ctx context.Context) ([]Device, error) {
	return call(ctx, e, func(inner *ffi.MobileEngine) ([]Device, error) {
		devices, err := inner.ListDevices()
		result := make([]Device, 0, len(devices))
		for _, device := range devices {
			result = append(result, Device{
				DeviceID:    device.DeviceId,
				DisplayName: device.DisplayName,
				IsLocal:     device.IsLocal,
				Online:      device.Online,
			})
		}
		return result, err
	})
}

// Invitation 是一次性邀请。内容属于敏感信息：String 与 GoString 固定脱敏，调用方不得记录其字段。
type Invitation struct {
	Code        string
	Full        string
	ExpiresAtMs int64
}

func (Invitation) String() string   { return "Invitation(REDACTED)" }
func (Invitation) GoString() string { return "Invitation(REDACTED)" }

// MarshalJSON 与 LogValue 同样固定脱敏，避免序列化或结构化日志意外带出邀请内容。
func (Invitation) MarshalJSON() ([]byte, error) { return []byte(`"REDACTED"`), nil }
func (Invitation) LogValue() slog.Value         { return slog.StringValue("REDACTED") }

// IssueInvitation 为当前空间签发邀请；没有可邀请的空间时返回 EngineError。
func (e *Engine) IssueInvitation(ctx context.Context) (Invitation, error) {
	return call(ctx, e, func(inner *ffi.MobileEngine) (Invitation, error) {
		issued, err := inner.IssueInvitation()
		return Invitation{Code: issued.InvitationCode, Full: issued.FullInvitation, ExpiresAtMs: issued.ExpiresAtMs}, err
	})
}

// LeaveSpace 退出当前空间；没有可退出的空间时返回 EngineError。
func (e *Engine) LeaveSpace(ctx context.Context) error {
	_, err := call(ctx, e, func(inner *ffi.MobileEngine) (struct{}, error) {
		return struct{}{}, inner.LeaveSpace()
	})
	return err
}

// NotifyConnectivityOpportunity 通知 Engine 前台、系统唤醒或网络变化。
// 非法的 Opportunity（包括零值）返回 ErrInvalidInput，不会进入 Rust。
func (e *Engine) NotifyConnectivityOpportunity(ctx context.Context, opportunity Opportunity) error {
	var reason ffi.ConnectivityOpportunity
	switch opportunity {
	case OpportunityForeground:
		reason = ffi.ConnectivityOpportunityForeground
	case OpportunitySystemWake:
		reason = ffi.ConnectivityOpportunitySystemWake
	case OpportunityNetworkChanged:
		reason = ffi.ConnectivityOpportunityNetworkChanged
	default:
		return ErrInvalidInput
	}
	_, err := call(ctx, e, func(inner *ffi.MobileEngine) (struct{}, error) {
		return struct{}{}, inner.NotifyConnectivityOpportunity(reason)
	})
	return err
}

// State 查询生命周期状态。
func (e *Engine) State(ctx context.Context) (State, error) {
	return call(ctx, e, func(inner *ffi.MobileEngine) (State, error) {
		state, err := inner.LifecycleState()
		return stateFrom(state), err
	})
}

// Suspend 把 Engine 暂停到可安全后台的状态；成功返回即暂停完成。
func (e *Engine) Suspend(ctx context.Context) error {
	_, err := call(ctx, e, func(inner *ffi.MobileEngine) (struct{}, error) {
		return struct{}{}, inner.Suspend()
	})
	return err
}

// Resume 从暂停恢复。
func (e *Engine) Resume(ctx context.Context) error {
	_, err := call(ctx, e, func(inner *ffi.MobileEngine) (struct{}, error) {
		return struct{}{}, inner.Resume()
	})
	return err
}

// NextEvent 阻塞到下一个事件、ctx 结束或 Engine 关闭。关闭后返回 ErrClosed。
func (e *Engine) NextEvent(ctx context.Context) (Event, error) {
	for {
		event, err := call(ctx, e, func(inner *ffi.MobileEngine) (Event, error) {
			next := inner.NextEvent(uint64(eventPollSlice / time.Millisecond))
			if next == nil {
				return nil, nil
			}
			return eventFrom(*next), nil
		})
		if err != nil {
			return nil, err
		}
		if event != nil {
			return event, nil
		}
	}
}

// Close 请求 Rust 在 deadline 内关闭并 join，排空在途调用后释放对象。
//
// 期限内未完成时返回 ErrCloseIncomplete（Rust 的可重试等待超时同样包装为它，并保留原始 EngineError），
// 此后新调用仍被拒绝，可再次调用 Close 继续收尾。Close 成功后重复调用返回 nil；并发的 Close 被串行化。
// 关闭期间已入队的事件对消费者不再可见：NextEvent 在 Close 开始后立即返回 ErrClosed。
// 负载下 Close 需要排空积压的在途调用（实测 32 个并发调用方时约 5 秒），期限应按负载放宽。
func (e *Engine) Close(deadline time.Duration) error {
	if deadline <= 0 {
		return ErrInvalidInput
	}
	e.closeMu.Lock()
	defer e.closeMu.Unlock()
	started := time.Now()
	e.mu.Lock()
	if e.closed {
		e.mu.Unlock()
		return nil
	}
	if !e.closing {
		e.closing = true
		if e.inflight == 0 {
			close(e.idle)
		}
	}
	e.mu.Unlock()

	// 不持锁：关闭会结束事件队列，唤醒阻塞中的 NextEvent。
	if err := e.inner.Shutdown(uint64(deadline / time.Millisecond)); err != nil {
		converted := convertError(err)
		if errors.Is(converted, ErrAlreadyStopped) {
			converted = nil
		}
		if converted != nil {
			var engineErr *EngineError
			if errors.As(converted, &engineErr) && engineErr.Retryable {
				// Rust 的等待超时（实测 1108）可重试：统一为可判定的 ErrCloseIncomplete，同时保留原始稳定错误。
				return fmt.Errorf("%w: %w", ErrCloseIncomplete, converted)
			}
			return converted
		}
	}

	select {
	case <-e.idle:
	default:
		timer := time.NewTimer(max(deadline-time.Since(started), 0))
		defer timer.Stop()
		select {
		case <-e.idle:
		case <-timer.C:
			return ErrCloseIncomplete
		}
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	if !e.closed {
		e.closed = true
		e.inner.Destroy()
	}
	return nil
}
