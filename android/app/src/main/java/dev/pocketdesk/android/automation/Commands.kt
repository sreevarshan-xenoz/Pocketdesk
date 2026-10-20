package dev.pocketdesk.android.automation

import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
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

    /** A session takes them while it runs, so the listener does not race it. */
    @Volatile
    var inSession = false

    /** The known automation apps installed. */
    fun installed(context: Context): Set<String> =
        CommandRule.KNOWN.filter { pkg ->
            runCatching { context.packageManager.getPackageInfo(pkg, 0) }.isSuccess
        }.toSet()

    /** Take the commands waiting; [own] is the computer the phone works with. */
    fun take(context: Context, own: HostStore.Host?) {
        val commands = RustCore.takeCommands()
        if (commands.isEmpty()) return
        val on = Prefs.commandsFromComputer(context)
        val to = CommandRule.onlyTo(Prefs.commandsOnlyTo(context), installed(context))
        for (command in commands) {
            val ownComputer = own != null && own.address == command.host && own.controlPort == command.port
            var decision = CommandRule.decide(on, ownComputer, withinBudget = true)
            if (decision == CommandRule.Decision.Send && !budget.take(SystemClock.elapsedRealtime())) {
                decision = CommandRule.Decision.TooMany
            }
            if (decision != CommandRule.Decision.Send) {
                Log.i("Commands", "a command from the computer was not sent on: $decision")
                continue
            }
            val intent =
                Intent(CommandRule.ACTION)
                    .putExtra("name", command.name)
                    .putExtra("text", command.text)
                    .putExtra("computer", command.host)
            to?.let { intent.setPackage(it) }
            runCatching { context.applicationContext.sendBroadcast(intent) }
                .onSuccess { Log.d("Commands", "a command went to ${to ?: "any app"}") }
                .onFailure { Log.w("Commands", "a command was not sent on: ${it.message}") }
        }
    }
}
