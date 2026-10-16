package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.TaskRun.Status
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TaskRunTest {
    @Test
    fun aRunWaitsThenRunsThenEnds() {
        assertEquals(Status.Waiting(asks = true), TaskRun.status(null, null, false, null, asks = true))
        assertEquals(Status.Running, TaskRun.status("running", null, false, null, asks = true))
        assertEquals(Status.Done(0), TaskRun.status("done", 0, false, null, asks = true))
        assertEquals(Status.Done(null), TaskRun.status("done", null, false, null, asks = false))
    }

    @Test
    fun aRunPastItsDeadlineIsSaidSo() {
        assertEquals(Status.StoppedAtDeadline, TaskRun.status("done", null, true, null, asks = false))
    }

    @Test
    fun aRefusalSaysWhy() {
        assertEquals(Status.Denied, TaskRun.status("refused", null, false, "denied", asks = true))
        assertEquals(Status.Busy, TaskRun.status("refused", null, false, "busy", asks = true))
        assertEquals(Status.Unknown, TaskRun.status("refused", null, false, "unknown", asks = true))
        assertEquals(Status.Unknown, TaskRun.status("refused", null, false, null, asks = true))
    }

    @Test
    fun aStartNothingAnswersIsGivenUp() {
        val asking = Status.Waiting(asks = true)
        assertEquals(asking, TaskRun.givenUp(asking, 60_000))
        assertEquals(Status.NoAnswer, TaskRun.givenUp(asking, 76_000))
        assertEquals(Status.NoAnswer, TaskRun.givenUp(Status.Waiting(asks = false), 21_000))
        assertEquals("a run is never given up", Status.Running, TaskRun.givenUp(Status.Running, 999_000))
        assertTrue(TaskRun.finished(Status.NoAnswer))
        assertTrue(TaskRun.finished(Status.NotSent("no link")))
    }

    @Test
    fun onlyAnEndedRunIsFinished() {
        assertFalse(TaskRun.finished(Status.Waiting(asks = true)))
        assertFalse(TaskRun.finished(Status.Running))
        assertTrue(TaskRun.finished(Status.Done(1)))
        assertTrue(TaskRun.finished(Status.Denied))
    }
}
