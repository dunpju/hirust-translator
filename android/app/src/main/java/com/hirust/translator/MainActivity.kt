package com.hirust.translator

import android.Manifest
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import uniffi.hirust_translator.HirustCallback
import uniffi.hirust_translator.HirustEngine
import java.io.File
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicBoolean

data class Utterance(val ts: String, val zh: String?, val en: String)

class MainActivity : ComponentActivity() {

    private var engine: HirustEngine? = null
    private val micRunning = AtomicBoolean(false)

    // Compose 状态（快照状态线程安全，可从回调线程写）
    private val phaseText = mutableStateOf("未启动")
    private val history = mutableStateOf(listOf<Utterance>())
    private val noticeText = mutableStateOf<String?>(null)
    private val modelReady = mutableStateOf(false)
    private val dlProgress = mutableStateOf<Pair<Long, Long>?>(null) // downloaded, total(-1未知)
    private val recording = mutableStateOf(false)
    private val accentUs = mutableStateOf(true)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val filesDir = filesDir.absolutePath
        val cb = object : HirustCallback {
            override fun onUtterance(ts: String, zh: String?, en: String, engine: String) {
                history.value = history.value + Utterance(ts, zh, en)
            }

            override fun onPhase(phase: String) {
                phaseText.value = phase
            }

            override fun onNotice(kind: Int, msg: String) {
                if (kind >= 1) noticeText.value = msg
            }

            override fun onMp3(bytes: ByteArray) {
                PlayerQueue.enqueue(this@MainActivity, bytes) { engine?.playbackDone() }
            }

            override fun onModelProgress(downloaded: ULong, total: ULong?) {
                dlProgress.value = downloaded.toLong() to (total?.toLong() ?: -1L)
            }
        }

        engine = HirustEngine("", filesDir, cb)
        modelReady.value = engine!!.modelReady()

        setContent {
            App()
        }
    }

    // ── 麦克风 ────────────────────────────────────────────────

    private fun startMic() {
        if (micRunning.getAndSet(true)) return
        val eng = engine ?: return
        Thread {
            try {
                val minBuf = AudioRecord.getMinBufferSize(
                    16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT
                )
                val rec = AudioRecord(
                    MediaRecorder.AudioSource.VOICE_RECOGNITION, 16000,
                    AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT,
                    maxOf(minBuf, 16000 * 4)
                )
                val buf = FloatArray(1600) // 100ms
                rec.startRecording()
                while (micRunning.get()) {
                    val n = rec.read(buf, 0, buf.size, AudioRecord.READ_BLOCKING)
                    if (n > 0) eng.feedPcm(buf.copyOf(n), 16000u)
                }
                rec.stop()
                rec.release()
            } catch (e: Exception) {
                noticeText.value = "麦克风错误: ${e.message}"
            } finally {
                micRunning.set(false)
            }
        }.start()
    }

    private fun stopMic() {
        micRunning.set(false)
    }

    // ── UI ────────────────────────────────────────────────────

    private val bg = Color(0xFF1E1F24)
    private val card = Color(0xFF26272E)
    private val cyan = Color(0xFF67E8F9)
    private val green = Color(0xFF22C55E)
    private val dim = Color(0xFF8A8F98)
    private val red = Color(0xFFEF4444)

    @Composable
    private fun App() {
        var hasAudioPermission by remember {
            mutableStateOf(
                ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                    == PackageManager.PERMISSION_GRANTED
            )
        }
        val permLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.RequestPermission()
        ) { granted ->
            hasAudioPermission = granted
            if (granted) toggleRecord(true)
        }

        MaterialTheme(colorScheme = darkColorScheme(background = bg, surface = card)) {
            Column(
                Modifier.fillMaxSize().background(bg).padding(16.dp)
            ) {
                // 顶部状态
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Hirust 中英同传", color = Color.White, fontSize = 18.sp)
                    Spacer(Modifier.weight(1f))
                    Text(phaseText.value, color = green, fontSize = 14.sp)
                    Spacer(Modifier.width(12.dp))
                    Text(if (accentUs.value) "🇺🇸 美式" else "🇬🇧 英式", color = cyan, fontSize = 14.sp)
                }
                Spacer(Modifier.height(8.dp))

                noticeText.value?.let {
                    Text("⚠ $it", color = Color(0xFFF59E0B), fontSize = 13.sp)
                    Spacer(Modifier.height(6.dp))
                }

                // 首次运行：模型下载
                if (!modelReady.value) {
                    Card(colors = CardDefaults.cardColors(containerColor = card)) {
                        Column(Modifier.padding(12.dp)) {
                            Text("首次运行需下载识别模型（base 约 142MB，hf-mirror 镜像）", color = dim, fontSize = 13.sp)
                            dlProgress.value?.let { (got, total) ->
                                Spacer(Modifier.height(6.dp))
                                if (total > 0) {
                                    LinearProgressIndicator(
                                        progress = { got.toFloat() / total },
                                        modifier = Modifier.fillMaxWidth()
                                    )
                                    Text("${got / 1000000} / ${total / 1000000} MB", color = dim, fontSize = 12.sp)
                                } else {
                                    LinearProgressIndicator(Modifier.fillMaxWidth())
                                }
                            } ?: run {
                                Spacer(Modifier.height(6.dp))
                                Button(onClick = { engine?.downloadModel() }) {
                                    Text("开始下载")
                                }
                            }
                        }
                    }
                }

                // 记录列表
                LazyColumn(
                    state = rememberLazyListState(),
                    modifier = Modifier.weight(1f).fillMaxWidth()
                ) {
                    items(history.value) { u ->
                        Column(
                            Modifier.fillMaxWidth()
                                .background(card, RoundedCornerShape(8.dp))
                                .padding(10.dp)
                        ) {
                            Row {
                                Text(u.ts, color = dim, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                                Spacer(Modifier.weight(1f))
                                Text("🔊", color = dim, fontSize = 13.sp)
                            }
                            u.zh?.let { Text("zh  $it", color = Color(0xFFE5E7EB), fontSize = 15.sp) }
                            Text("en  ${u.en}", color = cyan, fontSize = 15.sp)
                        }
                        Spacer(Modifier.height(8.dp))
                    }
                }

                Spacer(Modifier.height(8.dp))

                // 底部控制
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Button(
                        onClick = {
                            if (!hasAudioPermission) {
                                permLauncher.launch(Manifest.permission.RECORD_AUDIO)
                            } else {
                                toggleRecord(!recording.value)
                            }
                        },
                        colors = if (recording.value) ButtonDefaults.buttonColors(containerColor = red)
                        else ButtonDefaults.buttonColors(containerColor = green),
                        enabled = modelReady.value,
                        modifier = Modifier.weight(1f).height(48.dp)
                    ) {
                        Text(if (recording.value) "■ 停止录音" else "● 开始录音", fontSize = 16.sp)
                    }
                }
                Spacer(Modifier.height(8.dp))
                Row(horizontalArrangement = Arrangement.Center, modifier = Modifier.fillMaxWidth()) {
                    FilterChip(
                        selected = accentUs.value,
                        onClick = {
                            accentUs.value = !accentUs.value
                            engine?.setAccentUs(accentUs.value)
                        },
                        label = { Text("美式") })
                    Spacer(Modifier.width(8.dp))
                    FilterChip(
                        selected = !accentUs.value,
                        onClick = {
                            accentUs.value = !accentUs.value
                            engine?.setAccentUs(accentUs.value)
                        },
                        label = { Text("英式") })
                }
            }
        }
    }

    private fun toggleRecord(on: Boolean) {
        recording.value = on
        engine?.setPaused(!on)
        if (on) startMic() else stopMic()
    }

    override fun onDestroy() {
        super.onDestroy()
        stopMic()
        engine?.shutdown()
    }
}

/** 串行播放队列：mp3 → 临时文件 → MediaPlayer；播完回调 onDone 触发 Rust 解锁 */
object PlayerQueue {
    private val queue = ConcurrentLinkedQueue<ByteArray>()
    private val playing = AtomicBoolean(false)

    fun enqueue(ctx: ComponentActivity, bytes: ByteArray, onDone: () -> Unit) {
        queue.add(bytes)
        pump(ctx, onDone)
    }

    private fun pump(ctx: ComponentActivity, onDone: () -> Unit) {
        if (!playing.compareAndSet(false, true)) return
        val bytes = queue.poll()
        if (bytes == null) {
            playing.set(false)
            return
        }
        try {
            val f = File.createTempFile("tts_", ".mp3", ctx.cacheDir)
            f.writeBytes(bytes)
            val mp = MediaPlayer()
            mp.setDataSource(f.absolutePath)
            mp.setOnCompletionListener {
                it.release()
                f.delete()
                playing.set(false)
                onDone()
                pump(ctx, onDone)
            }
            mp.setOnErrorListener { p, _, _ ->
                p.release()
                f.delete()
                playing.set(false)
                onDone()
                pump(ctx, onDone)
                true
            }
            mp.prepare()
            mp.start()
        } catch (e: Exception) {
            playing.set(false)
            onDone()
        }
    }
}
