package dev.pocketdesk.android.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.TaskRun.Status
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** One of the computer's tasks, as the computer lists it. */
private data class Task(val name: String, val asks: Boolean)

/** A run in progress or done, and what came back. */
private data class Run(val task: Task, val id: String, val status: Status, val output: String, val truncated: Boolean)

/**
 * Named tasks (ecosystem 1c): the computer's task names (never their
 * commands); tapping one starts it and shows how it went.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TasksSheet(address: String, controlPort: Int, onDismiss: () -> Unit) {
    var tasks by remember { mutableStateOf<List<Task>?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var run by remember { mutableStateOf<Run?>(null) }

    LaunchedEffect(address, controlPort) {
        val listed = withContext(Dispatchers.IO) { RustCore.listTasks(address, controlPort) }
        listed
            .mapCatching { json ->
                val array = JSONObject(json).optJSONArray("tasks")
                (0 until (array?.length() ?: 0)).map { i ->
                    val task = array!!.getJSONObject(i)
                    Task(task.optString("name"), task.optBoolean("ask", true))
                }
            }.onSuccess { tasks = it }
            .onFailure { failure = it.message ?: it.toString() }
    }

    // Follow the run until it ends.
    val current = run
    LaunchedEffect(current?.id) {
        val started = current ?: return@LaunchedEffect
        while (isActive) {
            val json = withContext(Dispatchers.IO) { RustCore.taskResult(started.id) }
            val body = json.takeIf { it.isNotBlank() }?.let { runCatching { JSONObject(it) }.getOrNull() }
            val status =
                TaskRun.status(
                    state = body?.optString("state")?.takeIf { it.isNotEmpty() },
                    code = body?.takeIf { it.has("code") && !it.isNull("code") }?.optInt("code"),
                    timedOut = body?.optBoolean("timed_out") == true,
                    reason = body?.optString("reason")?.takeIf { it.isNotEmpty() },
                    asks = started.task.asks,
                )
            run = started.copy(
                status = status,
                output = body?.optString("output").orEmpty(),
                truncated = body?.optBoolean("truncated") == true,
            )
            if (TaskRun.finished(status)) break
            delay(1_000)
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 24.dp),
        ) {
            Text(stringResource(R.string.tasks_title), style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(R.string.tasks_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(bottom = 8.dp),
            )
            when {
                failure != null -> Text(stringResource(R.string.tasks_failed, failure.orEmpty()))
                tasks == null -> Text(stringResource(R.string.tasks_loading))
                tasks!!.isEmpty() -> Text(stringResource(R.string.tasks_none))
                else ->
                    tasks!!.forEach { task ->
                        ListItem(
                            headlineContent = { Text(task.name) },
                            supportingContent = {
                                if (task.asks) Text(stringResource(R.string.tasks_asks_first))
                            },
                            modifier = Modifier.padding(vertical = 2.dp),
                            trailingContent = {
                                androidx.compose.material3.TextButton(
                                    enabled = run?.let { TaskRun.finished(it.status) } ?: true,
                                    onClick = {
                                        val id = java.util.UUID.randomUUID().toString()
                                        run = Run(task, id, Status.Waiting(task.asks), "", false)
                                        Thread { RustCore.runTask(address, controlPort, task.name, id) }.start()
                                    },
                                ) { Text(stringResource(R.string.tasks_run)) }
                            },
                        )
                    }
            }
            run?.let { shown ->
                Text(
                    shown.task.name + " · " + statusText(shown.status),
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
                )
                if (shown.output.isNotEmpty()) {
                    if (shown.truncated) {
                        Text(
                            stringResource(R.string.tasks_output_cut),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Text(
                        shown.output,
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 320.dp)
                            .verticalScroll(rememberScrollState())
                            .horizontalScroll(rememberScrollState()),
                    )
                }
            }
        }
    }
}

@Composable
private fun statusText(status: Status): String =
    when (status) {
        is Status.Waiting ->
            if (status.asks) stringResource(R.string.tasks_waiting_owner) else stringResource(R.string.tasks_sent)
        Status.Running -> stringResource(R.string.tasks_running)
        is Status.Done ->
            status.code?.let { stringResource(R.string.tasks_done, it) } ?: stringResource(R.string.tasks_no_code)
        Status.StoppedAtDeadline -> stringResource(R.string.tasks_stopped)
        Status.Denied -> stringResource(R.string.tasks_denied)
        Status.Busy -> stringResource(R.string.tasks_busy)
        Status.Unknown -> stringResource(R.string.tasks_unknown)
    }
