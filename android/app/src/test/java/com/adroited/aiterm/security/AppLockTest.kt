package com.adroited.aiterm.security

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The five-minute background lock. The biometric prompt itself needs a device,
 * but the decision of *when* to demand it is plain arithmetic and is pinned
 * down here.
 */
class AppLockTest {

    private var now = 1_000_000L
    private val lock = AppLock(clock = { now })

    @Test
    fun freshlyStartedApp_isNotLocked() {
        assertFalse(lock.isLocked.value)
    }

    @Test
    fun fiveMinutesInTheBackground_locksTheApp() {
        lock.onEnterBackground()
        now += AppLock.BACKGROUND_LOCK_TIMEOUT_MILLIS
        lock.onEnterForeground()

        assertTrue(lock.isLocked.value)
    }

    @Test
    fun aShortTripToTheBackground_doesNotLockTheApp() {
        lock.onEnterBackground()
        now += AppLock.BACKGROUND_LOCK_TIMEOUT_MILLIS - 1
        lock.onEnterForeground()

        assertFalse(lock.isLocked.value)
    }

    @Test
    fun unlocking_clearsTheLockAndRestartsTheClock() {
        lock.onEnterBackground()
        now += AppLock.BACKGROUND_LOCK_TIMEOUT_MILLIS
        lock.onEnterForeground()
        lock.unlock()

        assertFalse(lock.isLocked.value)

        lock.onEnterBackground()
        now += 1_000
        lock.onEnterForeground()

        assertFalse(lock.isLocked.value)
    }

    @Test
    fun lockNow_locksWithoutWaiting() {
        lock.lockNow()

        assertTrue(lock.isLocked.value)
    }

    @Test
    fun repeatedForegrounding_withoutBackgrounding_doesNotLock() {
        lock.onEnterForeground()
        now += AppLock.BACKGROUND_LOCK_TIMEOUT_MILLIS * 10
        lock.onEnterForeground()

        assertFalse(lock.isLocked.value)
    }

    @Test
    fun monotonicClockMovingBackwards_locksFailClosed() {
        lock.onEnterBackground()
        now -= 1

        lock.onEnterForeground()

        assertTrue(lock.isLocked.value)
    }
}
