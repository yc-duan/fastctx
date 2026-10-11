package dev.fastctx.world

import android.app.Activity
import android.content.Intent
import android.provider.DocumentsContract
import android.system.Os
import android.system.StructStat
import android.webkit.MimeTypeMap
import androidx.activity.result.ActivityResult
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.io.IOException
import java.io.FileInputStream
import java.util.concurrent.Executors

/** A document provider owns the destination; no filesystem path is assumed. */
object Saving {
    private val copies = Executors.newSingleThreadExecutor()

    fun failed(invoke: Invoke, error: Exception) {
        invoke.resolve(JSObject().put("kind", "failed").put("reason", error.toString()))
    }

    private fun source(invoke: Invoke): File {
        val supplied = File(invoke.getArgs().getString("path"))
        require(supplied.isAbsolute) { "The source path must be absolute" }
        val file = supplied.canonicalFile
        require(file.isFile) { "The source must be an ordinary file" }
        return file
    }

    class Request(val intent: Intent, val file: File, val input: FileInputStream, val before: StructStat, val modified: Long) {
        fun close() = input.close()
    }

    fun request(invoke: Invoke): Request {
        val name = invoke.getArgs().getString("suggestedName")
        require(name.isNotEmpty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\') && !name.contains('\u0000')) {
            "The suggested name must be a single filename"
        }
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(File(name).extension.lowercase())
            ?: "application/octet-stream"
        val file = source(invoke)
        val input = file.inputStream()
        try {
            val before = Os.fstat(input.fd)
            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mime).putExtra(Intent.EXTRA_TITLE, name)
            return Request(intent, file, input, before, file.lastModified())
        } catch (error: Exception) { input.close(); throw error }
    }

    fun complete(invoke: Invoke, request: Request, result: ActivityResult) {
        if (result.resultCode == Activity.RESULT_CANCELED) {
            request.close()
            invoke.resolve(JSObject().put("kind", "cancelled")); return
        }
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) {
            request.close()
            failed(invoke, IOException("The document creator returned no destination (result ${result.resultCode})")); return
        }
        val resolver = WorldRuntime.context.contentResolver
        copies.execute {
            try {
                request.input.use { input ->
                    val output = resolver.openOutputStream(uri, "w")
                        ?: throw IOException("The document provider did not open an output stream")
                    val bytes = output.use { stream ->
                        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                        var bytes = 0L
                        while (bytes < request.before.st_size) {
                            val want = minOf(buffer.size.toLong(), request.before.st_size - bytes).toInt()
                            val read = input.read(buffer, 0, want)
                            if (read <= 0) throw IOException("The source changed while saving")
                            stream.write(buffer, 0, read)
                            bytes += read
                        }
                        stream.flush()
                        bytes
                    }
                    val after = Os.fstat(input.fd)
                    val current = Os.stat(request.file.path)
                    fun unchanged(stat: StructStat) = stat.st_dev == request.before.st_dev && stat.st_ino == request.before.st_ino &&
                        stat.st_size == request.before.st_size && stat.st_mtime == request.before.st_mtime && stat.st_ctime == request.before.st_ctime
                    check(bytes == request.before.st_size && unchanged(after) && unchanged(current) && request.file.lastModified() == request.modified) {
                        "The source changed while saving"
                    }
                }
                invoke.resolve(JSObject().put("kind", "saved"))
            } catch (error: Exception) {
                try {
                    if (!DocumentsContract.deleteDocument(resolver, uri)) {
                        error.addSuppressed(IOException("The incomplete document could not be deleted"))
                    }
                } catch (cleanup: Exception) { error.addSuppressed(cleanup) }
                val reason = error.toString() + error.suppressed.joinToString("") { "; $it" }
                invoke.resolve(JSObject().put("kind", "failed").put("reason", reason))
            }
        }
    }
}
