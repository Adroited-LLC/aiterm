package com.adroited.aiterm

import android.app.Application

/**
 * Process-wide entry point. Deliberately empty for now: the pairing store
 * (Task 8) and the remote client (Task 9) get constructed here once they exist,
 * so their lifetime is the process and not a single Activity.
 */
class AitermApplication : Application()
