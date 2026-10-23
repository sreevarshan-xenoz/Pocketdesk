package dev.pocketdesk.android.ui

import android.Manifest
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.R
import dev.pocketdesk.android.compat.Android17
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.TextPrefs

/**
 * Settings: texts on the computer (N5b). "Show texts" asks for RECEIVE_SMS,
 * "Reply from my computer" for SEND_SMS, each only when turned on. Replies
 * also need notification access, whose service keeps the connection they
 * arrive on; the row says so when it is missing.
 */
@Composable
fun TextsSection() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    // Read again on return: access may have been granted in Android's settings.
    val resumes = rememberResumes()
    val show = (version + resumes).let { TextPrefs.show(context) }
    val reply = (version + resumes).let { TextPrefs.reply(context) }
    val canReceive = (version + resumes).let { TextPrefs.granted(context, Manifest.permission.RECEIVE_SMS) }
    val canSend = (version + resumes).let { TextPrefs.granted(context, Manifest.permission.SEND_SMS) }
    val names = (version + resumes).let { TextPrefs.names(context) }
    val canReadContacts = (version + resumes).let { TextPrefs.granted(context, Manifest.permission.READ_CONTACTS) }
    val listening = (version + resumes).let { PhoneNotificationPrefs.accessGranted(context) }

    val askReceive =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setShow(context, granted)
            version++
        }
    val askSend =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setReply(context, granted)
            version++
        }

    val askContacts =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setNames(context, granted)
            version++
        }
    // P10: the contacts themselves to the computer, asking for them too.
    val contactsOn = (version + resumes).let { TextPrefs.contactsToComputer(context) }
    val askContactsList =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            TextPrefs.setContactsToComputer(context, granted)
            version++
        }

    fun setContactsOn(on: Boolean) {
        if (on && !canReadContacts) {
            askContactsList.launch(Manifest.permission.READ_CONTACTS)
        } else {
            TextPrefs.setContactsToComputer(context, on)
            version++
        }
    }

    fun setNames(on: Boolean) {
        if (on && !canReadContacts) {
            askContacts.launch(Manifest.permission.READ_CONTACTS)
        } else {
            TextPrefs.setNames(context, on)
            version++
        }
    }

    fun setShow(on: Boolean) {
        if (on && !canReceive) {
            askReceive.launch(Manifest.permission.RECEIVE_SMS)
        } else {
            TextPrefs.setShow(context, on)
            version++
        }
    }

    fun setReply(on: Boolean) {
        if (on && !canSend) {
            askSend.launch(Manifest.permission.SEND_SMS)
        } else {
            TextPrefs.setReply(context, on)
            // Turned off: requests waiting from before are never sent later.
            if (!on) dev.pocketdesk.android.bridge.RustCore.takeSmsRequests()
            version++
        }
    }

    SettingSwitch(
        stringResource(R.string.texts_show),
        // Android 17 holds texts with a code for 3 hours (N4).
        if (Android17.holdsCodeTexts(android.os.Build.VERSION.SDK_INT)) {
            stringResource(R.string.texts_show_summary_17)
        } else {
            stringResource(R.string.texts_show_summary)
        },
        show && canReceive,
        ::setShow,
    )
    val needsAccess = reply && canSend && !listening
    SettingSwitch(
        stringResource(R.string.texts_reply),
        stringResource(if (needsAccess) R.string.texts_reply_needs_access else R.string.texts_reply_summary),
        reply && canSend,
        ::setReply,
        warning = needsAccess,
    )
    if (needsAccess) AllowNotificationAccess()
    SettingSwitch(
        stringResource(R.string.texts_names),
        stringResource(R.string.texts_names_summary),
        names && canReadContacts,
        ::setNames,
    )
    SettingSwitch(
        stringResource(R.string.texts_contacts),
        stringResource(R.string.texts_contacts_summary),
        contactsOn && canReadContacts,
        ::setContactsOn,
    )
}
