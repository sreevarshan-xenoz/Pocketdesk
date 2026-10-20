package dev.pocketdesk.android.automation

import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.NotificationActionRule

/**
 * Commands the computer sends the phone's automation apps (L6), broadcast
 * as [CommandRule.ACTION] with `name` and `text`, to the "Only to" app
 * when one is set and installed.
 */
object Commands {
    private val budget = NotificationActionRule.Budget(CommandRule.PER_MINUTE)

    /** Sessions running, which take the commands (L6 review: a flag was
     * cleared by the old of two sessions overlapping). */
    val sessions = java.util.concurrent.atomic.AtomicInteger(0)

    /** The known automation apps installed. */
    fun installed(context: Context): Set<String> =
        CommandRule.KNOWN.filter { pkg ->
            runCatching { context.packageManager.getPackageInfo(pkg, 0) }.isSuccess
        }.toSet()

    /**
     * Take the commands waiting; [isOwn] says whether a command's computer
     * (its address and port) is the phone's own.
     */
    fun take(context: Context, isOwn: (String, Int) -> Boolean) {
        val commands = RustCore.takeCommands()
        if (commands.isEmpty()) return
        val on = Prefs.commandsFromComputer(context)
        val to = CommandRule.onlyTo(Prefs.commandsOnlyTo(context), installed(context))
        for (command in commands) {
            val ownComputer = isOwn(command.host, command.port)
            var decision = CommandRule.decide(on, ownComputer, withinBudget = true)
            if (decision == CommandRule.Decision.Send && !budget.take(SystemClock.elapsedRealtime())) {
                decision = CommandRule.Decision.TooMany
            }
            if (decision != CommandRule.Decision.Send) {
                Log.i("Commands", "a command from the computer was not sent on: $decision")
                continue
            }
            // Never to any app (L6 review): a script's text may be a secret.
            if (to == null) {
                Log.i("Commands", "a command from the computer was not sent on: no automation app")
                continue
            }
            val intent =
                Intent(CommandRule.ACTION)
                    // Named as the phone's other broadcasts' extras are (1f).
                    .putExtra("dev.pocketdesk.android.extra.NAME", command.name)
                    .putExtra("dev.pocketdesk.android.extra.TEXT", command.text)
                    .putExtra("dev.pocketdesk.android.extra.ADDRESS", command.host)
            intent.setPackage(to)
            runCatching { context.applicationContext.sendBroadcast(intent) }
                .onSuccess { Log.d("Commands", "a command went to $to") }
                .onFailure { Log.w("Commands", "a command was not sent on: ${it.message}") }
        }
    }
}
