package com.zplayer.music;

import android.app.NativeActivity;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.PixelFormat;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;

import androidx.core.splashscreen.SplashScreen;

/**
 * 自定义启动 Activity（继承 NativeActivity），用于消除 Slint 的 SurfaceView 黑帧。
 *
 * 实测结论（重要）：
 * 1) 不能用 SplashScreen 的 setKeepOnScreenCondition 保持系统启动屏——会阻止窗口绘制，
 *    进而阻止 SurfaceView 建立 surface，Slint 无法出首帧，形成死锁。
 * 2) 普通的 View / ImageView 覆盖层也无效：Slint 的 SurfaceView 的 surface 位于窗口之上，
 *    普通窗口内视图盖不住它（置顶、换父容器都试过，仍透黑）。
 *
 * 唯一可靠做法：用**另一个 ZOrderOnTop 的 SurfaceView** 作为"遮盖层"，直接在其 surface
 * 上画启动底色 + logo。ZOrderOnTop 保证它位于主渲染 surface 之上；待 Rust 侧渲染器
 * 首个渲染通知（JNI onFirstFrameRendered）到达后移除遮盖层，露出 Slint 启动动画。
 *
 * 该方案只依赖原生视图/surface 层级，与渲染后端解耦，iOS 可复用"首帧回调再撤遮盖"。
 */
public class MainActivity extends NativeActivity {

    private static final String TAG = "zplayer";
    private static final long RESUME_HIDE_DELAY_MS = 260L;
    private static final long SAFETY_DISMISS_MS = 6000L;

    private volatile boolean firstFrameRendered = false;
    private volatile int veilColor = 0xFF171329;

    private SurfaceView veil;
    private Bitmap logoBitmap;
    private SurfaceHolder surfaceHolder;
    private View decorView;

    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private int coverToken = 0;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        final SplashScreen splashScreen = SplashScreen.installSplashScreen(this);
        super.onCreate(savedInstanceState);
        // 明确不保持系统启动屏（保持会阻塞 surface 建立，导致 Slint 死锁）
        splashScreen.setKeepOnScreenCondition(() -> false);

        decorView = getWindow().getDecorView();
        logoBitmap = loadSplashLogoBitmap();

        // 遮盖层：ZOrderOnTop 的 SurfaceView，位于主渲染 surface 之上
        veil = new SurfaceView(this);
        veil.setZOrderOnTop(true);
        veil.getHolder().setFormat(PixelFormat.RGBA_8888);
        veil.getHolder().addCallback(new SurfaceHolder.Callback() {
            @Override
            public void surfaceCreated(SurfaceHolder holder) {
                paintVeil(holder);
            }

            @Override
            public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
                paintVeil(holder);
            }

            @Override
            public void surfaceDestroyed(SurfaceHolder holder) {
            }
        });

        ViewGroup contentRoot = findViewById(android.R.id.content);
        ViewGroup parent = contentRoot != null
                ? contentRoot
                : (decorView instanceof ViewGroup ? (ViewGroup) decorView : null);
        if (parent != null) {
            parent.addView(veil, new ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT));
            veil.bringToFront();
        }

        // 监听主渲染 SurfaceView 的 surface 生命周期，用于切回前台时决定何时撤遮盖
        SurfaceView mainSurface = findMainSurfaceView(decorView, veil);
        if (mainSurface != null) {
            surfaceHolder = mainSurface.getHolder();
            surfaceHolder.addCallback(new SurfaceHolder.Callback() {
                @Override
                public void surfaceCreated(SurfaceHolder holder) {
                    // 切回前台 surface 被系统重建：先盖住（覆盖黑 surface），随后撤除
                    if (firstFrameRendered) {
                        showVeil();
                        // 兜底：即使渲染回调迟迟不来，也在 2s 后撤除，避免遮盖卡住
                        scheduleHideVeil(2000L);
                    }
                }

                @Override
                public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
                    // 撤遮盖由 Rust 的 AfterRendering 回调驱动，这里不再用固定延时
                }

                @Override
                public void surfaceDestroyed(SurfaceHolder holder) {
                }
            });
        }

        mainHandler.postDelayed(this::forceDismissVeil, SAFETY_DISMISS_MS);
    }

    @Override
    protected void onResume() {
        super.onResume();
    }

    @SuppressWarnings("unused")
    public void onFirstFrameRendered() {
        // 会被 Rust 在每次 AfterRendering 节流回调：仅在遮盖可见时撤除，天然幂等
        firstFrameRendered = true;
        final int token = ++coverToken;
        mainHandler.post(() -> {
            if (token == coverToken && veil != null && veil.getVisibility() == View.VISIBLE) {
                Log.i(TAG, "[boot] 撤除启动遮盖（真实帧已绘制）");
                veil.setVisibility(View.GONE);
            }
        });
    }

    @SuppressWarnings("unused")
    public void setThemeBackground(final int argb) {
        veilColor = argb;
        mainHandler.post(() -> {
            try {
                getWindow().setBackgroundDrawable(new ColorDrawable(argb));
            } catch (Throwable ignored) {
            }
            if (veil != null && veil.getHolder().getSurface().isValid()) {
                paintVeil(veil.getHolder());
            }
        });
    }

    /** 由 Rust 调用（JNI）：用系统浏览器打开链接（更新下载页 / MV 页面）。 */
    @SuppressWarnings("unused")
    public void openUrl(final String url) {
        mainHandler.post(() -> {
            try {
                Intent it = new Intent(Intent.ACTION_VIEW, Uri.parse(url));
                it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                startActivity(it);
            } catch (Throwable e) {
                Log.w(TAG, "[ui] 打开链接失败: " + e);
            }
        });
    }

    private void forceDismissVeil() {
        if (firstFrameRendered) {
            return;
        }
        Log.w(TAG, "[boot] 启动遮盖超时兜底撤除（未收到首帧回调）");
        firstFrameRendered = true;
        if (veil != null) {
            veil.setVisibility(View.GONE);
        }
    }

    private void showVeil() {
        final int token = ++coverToken;
        mainHandler.post(() -> {
            if (token != coverToken || veil == null) {
                return;
            }
            veil.setVisibility(View.VISIBLE);
            if (veil.getHolder().getSurface().isValid()) {
                paintVeil(veil.getHolder());
            }
            if (veil.getParent() instanceof ViewGroup) {
                ((ViewGroup) veil.getParent()).bringChildToFront(veil);
            }
        });
    }

    private void scheduleHideVeil(final long delayMs) {
        final int token = ++coverToken;
        mainHandler.postDelayed(() -> {
            if (token == coverToken && veil != null) {
                veil.setVisibility(View.GONE);
            }
        }, delayMs);
    }

    /** 把启动底色 + logo 画到遮盖层 surface 上。 */
    private void paintVeil(SurfaceHolder holder) {
        Canvas canvas = null;
        try {
            canvas = holder.lockCanvas();
            if (canvas == null) {
                return;
            }
            canvas.drawColor(veilColor);
            if (logoBitmap != null) {
                int x = (canvas.getWidth() - logoBitmap.getWidth()) / 2;
                int y = (canvas.getHeight() - logoBitmap.getHeight()) / 2;
                canvas.drawBitmap(logoBitmap, x, y, null);
            }
        } catch (Throwable e) {
            Log.w(TAG, "[boot] 绘制启动遮盖失败: " + e);
        } finally {
            if (canvas != null) {
                try {
                    holder.unlockCanvasAndPost(canvas);
                } catch (Throwable ignored) {
                }
            }
        }
    }

    private Bitmap loadSplashLogoBitmap() {
        try {
            int id = getResources().getIdentifier("splash_logo", "drawable", getPackageName());
            if (id == 0) {
                return null;
            }
            Drawable d = getResources().getDrawable(id, getTheme());
            if (d == null) {
                return null;
            }
            int w = d.getIntrinsicWidth();
            int h = d.getIntrinsicHeight();
            if (w <= 0 || h <= 0) {
                return null;
            }
            // 统一按 288dp 画布缩放（logo 本身 144dp），避免不同密度源图被画得过大
            float density = getResources().getDisplayMetrics().density;
            int targetW = Math.round(288f * density);
            int targetH = Math.max(1, Math.round((float) h * targetW / w));
            Bitmap bitmap = Bitmap.createBitmap(targetW, targetH, Bitmap.Config.ARGB_8888);
            Canvas c = new Canvas(bitmap);
            d.setBounds(0, 0, targetW, targetH);
            d.draw(c);
            return bitmap;
        } catch (Throwable ignored) {
            return null;
        }
    }

    /** 找到主渲染 SurfaceView（排除我们的遮盖 SurfaceView）。 */
    private static SurfaceView findMainSurfaceView(View root, SurfaceView exclude) {
        if (root instanceof SurfaceView && root != exclude) {
            return (SurfaceView) root;
        }
        if (root instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) root;
            for (int i = 0; i < group.getChildCount(); i++) {
                SurfaceView found = findMainSurfaceView(group.getChildAt(i), exclude);
                if (found != null) {
                    return found;
                }
            }
        }
        return null;
    }
}
