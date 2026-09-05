package com.adroited.aiterm.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.imeNestedScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.absolutePadding
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.FocusOwner
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteUploadException
import com.adroited.aiterm.remote.RemoteUploadProgress
import com.adroited.aiterm.remote.TerminalSize
import com.adroited.aiterm.terminal.CellAttributes
import com.adroited.aiterm.terminal.CursorShape
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenSnapshot
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.TerminalColor
import kotlinx.coroutines.launch
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import java.io.File
import java.net.URI

@Composable
fun RemoteTerminalScreen(
    viewModel: RemoteTerminalViewModel,
    onBack: () -> Unit,
    keyBarPreference: TerminalKeyBarPreference,
) {
    val state by viewModel.client.state.collectAsStateWithLifecycle()
    val screen by viewModel.client.screen.collectAsStateWithLifecycle()
    val scrollback by viewModel.client.scrollback.collectAsStateWithLifecycle()
    val keyBarExpanded by keyBarPreference.expanded.collectAsStateWithLifecycle()
    TerminalScreenContent(
        state = state,
        screen = screen,
        scrollback = scrollback,
        keyBarExpanded = keyBarExpanded,
        onKeyBarExpandedChange = keyBarPreference::setExpanded,
        onBack = onBack,
        onReconnect = viewModel::reconnect,
        onInput = viewModel::sendInput,
        onInputBatch = viewModel::submitInputs,
        draftStore = viewModel.terminalDrafts,
        onUploadImages = viewModel::uploadDraftImages,
        onTakeFocus = viewModel::takeFocus,
        onResize = viewModel::resize,
        onLoadScrollback = viewModel::loadOlderScrollback,
        onRestartScrollback = { viewModel.client.restartScrollback() },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TerminalScreenContent(
    state: RemoteClientState,
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow> = emptyList(),
    keyBarExpanded: Boolean = true,
    onKeyBarExpandedChange: (Boolean) -> Unit = {},
    onBack: () -> Unit = {},
    onReconnect: () -> Unit = {},
    onInput: (String) -> Unit = {},
    onInputBatch: (suspend (String, List<String>) -> Boolean)? = null,
    draftStore: TerminalDraftStore? = null,
    imagePickerLauncher: TerminalImagePickerLauncher? = null,
    imageNormalizer: TerminalImageNormalization? = null,
    onUploadImages: suspend (
        String,
        List<TerminalAttachmentImage>,
        (RemoteUploadProgress) -> Unit,
    ) -> Result<List<String>> = { _, _, _ ->
        Result.failure(IllegalStateException("Image upload is unavailable."))
    },
    onTakeFocus: (Int, Int) -> Unit = { _, _ -> },
    onResize: (Int, Int) -> Unit = { _, _ -> },
    onLoadScrollback: () -> Unit = {},
    onRestartScrollback: () -> Unit = {},
    imeInsets: WindowInsets = WindowInsets.ime,
    navigationInsets: WindowInsets = WindowInsets.navigationBars,
) {
    val coroutineScope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val lifecycleState by lifecycle.currentStateFlow.collectAsStateWithLifecycle()
    val resumed = lifecycleState.isAtLeast(Lifecycle.State.RESUMED)
    var viewportReady by remember { mutableStateOf(false) }
    var cols by remember { mutableIntStateOf(screen?.cols ?: 80) }
    var rows by remember { mutableIntStateOf(screen?.rows ?: 24) }
    val terminalMetrics = rememberTerminalMetrics()
    val inputFocus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val context = LocalContext.current
    val density = LocalDensity.current
    val layoutDirection = LocalLayoutDirection.current
    val localDraftStore = remember { TerminalDraftStore() }
    val activeDraftStore = draftStore ?: localDraftStore
    val allDrafts by activeDraftStore.drafts.collectAsStateWithLifecycle()
    val latestScreen by rememberUpdatedState(screen)
    val activeTabId = screen?.tabId
    val tabDraft = activeTabId?.let { allDrafts[it] } ?: TerminalTabDraft()
    val composer = tabDraft.composer
    val composerVisible = keyBarExpanded && composer.expanded
    val attachments = tabDraft.attachments
    val defaultNormalizer = remember(context) { TerminalImageNormalizer(context) }
    val normalizer = imageNormalizer ?: remember(defaultNormalizer) {
        TerminalImageNormalization(defaultNormalizer::normalize)
    }
    var showImageSources by remember { mutableStateOf(false) }
    var showDiscardDrafts by remember { mutableStateOf(false) }
    var chromeInteractiveHeightPx by remember(screen?.tabId) { mutableIntStateOf(0) }
    val bottomInsets = imeInsets.union(navigationInsets)
    val bottomInsetPx = bottomInsets.getBottom(density)
    val navigationLeftInsetPx = navigationInsets.getLeft(density, layoutDirection)
    val navigationRightInsetPx = navigationInsets.getRight(density, layoutDirection)
    val onViewportSizeChanged = remember {
        { size: TerminalSize ->
            cols = size.cols
            rows = size.rows
            viewportReady = true
        }
    }
    val onRequestKeyboard = remember(
        state.focus, activeTabId, activeDraftStore, composerVisible, onKeyBarExpandedChange, keyboard,
    ) {
        {
            if (state.focus == FocusOwner.Self && activeTabId != null) {
                onKeyBarExpandedChange(true)
                activeDraftStore.updateComposer(activeTabId) { it.open() }
                if (composerVisible) {
                    inputFocus.requestFocus()
                    keyboard?.show()
                }
            }
        }
    }

    fun setAttachmentMessage(tabId: String, message: String) {
        activeDraftStore.updateAttachments(tabId) { it.copy(message = message) }
    }

    fun handlePickerResult(tabId: String, result: TerminalImagePickerResult) {
        when (result) {
            TerminalImagePickerResult.Cancelled -> Unit
            is TerminalImagePickerResult.Failed -> setAttachmentMessage(tabId, result.message)
            is TerminalImagePickerResult.Selected -> coroutineScope.launch(start = CoroutineStart.UNDISPATCHED) {
                val preparation = activeDraftStore.transitionAttachments(tabId) { it.beginPreparation() }
                if (!preparation.accepted) {
                    result.ownedCaptureFiles.forEach(File::delete)
                    return@launch
                }
                try {
                    val currentCount = activeDraftStore.draftFor(tabId).attachments.items.size
                    val remaining = (TerminalAttachmentDraft.MAX_IMAGES - currentCount).coerceAtLeast(0)
                    val distinctUris = result.uris.distinct()
                    var selectionMessage: String? = null
                    for (uri in distinctUris.take(remaining)) {
                        val normalizedImage = normalizer.normalize(uri).getOrElse { error ->
                            selectionMessage = terminalImageErrorMessage(error)
                            continue
                        }
                        val transition = activeDraftStore.transitionAttachments(tabId) {
                            it.add(normalizedImage)
                        }
                        if (!transition.accepted) {
                            selectionMessage = transition.draft.message
                            normalizedImage.file.delete()
                        }
                    }
                    val finalMessage = when {
                        result.uris.size != distinctUris.size ->
                            "This image is already attached."
                        distinctUris.size > remaining ->
                            "You can attach up to 4 images."
                        else -> selectionMessage
                    }
                    finalMessage?.let { setAttachmentMessage(tabId, it) }
                } finally {
                    result.ownedCaptureFiles.forEach(File::delete)
                    activeDraftStore.transitionAttachments(tabId) { it.finishPreparation() }
                }
            }
        }
    }
    val nativePicker = rememberTerminalImagePicker(::handlePickerResult)
    val picker = imagePickerLauncher ?: nativePicker

    fun submitComposer() {
        val activeScreen = screen ?: return
        val tabId = activeScreen.tabId
        val current = activeDraftStore.draftFor(tabId)
        if (current.attachments.preparing || current.attachments.submitting ||
            (current.composer.value.text.isEmpty() && current.attachments.items.isEmpty())
        ) return
        if (current.attachments.items.isEmpty()) {
            val outbound = formatTerminalSubmission(
                text = current.composer.value.text,
                paths = emptyList(),
                bracketedPaste = activeScreen.modes.bracketedPaste,
            )
            activeDraftStore.transitionAttachments(tabId) {
                TerminalAttachmentDraftUpdate(it.copy(submitting = true, message = null))
            }
            coroutineScope.launch {
                try {
                    val accepted = onInputBatch?.invoke(tabId, outbound) ?: run {
                        outbound.forEach(onInput)
                        true
                    }
                    if (accepted) {
                        activeDraftStore.clear(tabId)
                        keyboard?.hide()
                    } else {
                        setAttachmentMessage(
                            tabId,
                            "Terminal input was not accepted. Take focus and try again.",
                        )
                    }
                } finally {
                    activeDraftStore.transitionAttachments(tabId) {
                        TerminalAttachmentDraftUpdate(it.copy(submitting = false))
                    }
                }
            }
            return
        }
        coroutineScope.launch {
            var uploadBegan = false
            try {
                val initial = activeDraftStore.draftFor(tabId)
                val images = initial.attachments.items.map { it.image }
                if (images.isEmpty()) return@launch
                val began = activeDraftStore.transitionAttachments(tabId) { it.beginSubmission() }
                if (!began.accepted) return@launch
                uploadBegan = true
                val paths = onUploadImages(tabId, images) { progress ->
                    activeDraftStore.transitionAttachments(tabId) {
                        it.recordProgress(progress.sourceId, progress.sent, progress.total)
                    }
                }.getOrElse { error ->
                    activeDraftStore.transitionAttachments(tabId) {
                        it.failSubmission(terminalUploadErrorMessage(error))
                    }
                    uploadBegan = false
                    return@launch
                }
                val latest = activeDraftStore.draftFor(tabId)
                val submissionScreen = latestScreen
                if (submissionScreen?.tabId != tabId) {
                    activeDraftStore.transitionAttachments(tabId) {
                        it.failSubmission("Terminal tab changed while images were uploading. Try again.")
                    }
                    uploadBegan = false
                    return@launch
                }
                val outbound = formatTerminalSubmission(
                    text = latest.composer.value.text,
                    paths = paths,
                    bracketedPaste = submissionScreen.modes.bracketedPaste,
                )
                val accepted = onInputBatch?.invoke(tabId, outbound) ?: run {
                    outbound.forEach(onInput)
                    true
                }
                if (!accepted) {
                    val message = "Terminal input was not accepted. Take focus and try again."
                    activeDraftStore.transitionAttachments(tabId) { it.failSubmission(message) }
                    uploadBegan = false
                    return@launch
                }
                val completed = activeDraftStore.completeSubmission(tabId)
                completed.removed.forEach { it.image.file.delete() }
                uploadBegan = false
                keyboard?.hide()
            } catch (cancelled: CancellationException) {
                if (uploadBegan) {
                    activeDraftStore.transitionAttachments(tabId) {
                        it.failSubmission("Image upload paused. Try again.")
                    }
                }
                throw cancelled
            }
        }
    }

    fun requestLeave() {
        if (activeDraftStore.hasDrafts()) showDiscardDrafts = true else onBack()
    }

    BackHandler {
        if (composerVisible && activeTabId != null) {
            activeDraftStore.updateComposer(activeTabId) { it.close() }
            keyboard?.hide()
        } else {
            requestLeave()
        }
    }
    // Claim once on entry, foreground return, or a newly attached screen. Do not
    // react to ownership changes: a deliberate desktop click must not start a focus fight.
    LaunchedEffect(resumed, state.connection, screen?.tabId, viewportReady) {
        if (resumed && viewportReady && state.connection == ConnectionState.Connected &&
            screen != null && state.focus != FocusOwner.Self
        ) {
            onTakeFocus(cols, rows)
        }
    }
    LaunchedEffect(composerVisible, state.focus, screen?.tabId) {
        if (composerVisible && state.focus == FocusOwner.Self && screen != null) {
            inputFocus.requestFocus()
            keyboard?.show()
        }
    }

    LaunchedEffect(keyBarExpanded) {
        if (!keyBarExpanded) keyboard?.hide()
    }

    Scaffold(
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = ::requestLeave) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                title = {
                    Column {
                        Text(state.activeTitle ?: "Remote terminal")
                        Text(
                            state.connection.label(),
                            style = MaterialTheme.typography.labelMedium,
                            color = state.connection.color(),
                        )
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(top = padding.calculateTopPadding())) {
            Column(Modifier.fillMaxSize().testTag("terminal-surface")) {
                ConnectionRail(state, onReconnect)
                TerminalViewport(
                    screen = screen,
                    scrollback = scrollback,
                    scrollbackLoading = state.scrollbackLoading,
                    scrollbackHasMore = state.scrollbackHasMore,
                    scrollbackError = state.scrollbackError,
                    onLoadScrollback = onLoadScrollback,
                    onRestartScrollback = onRestartScrollback,
                    metrics = terminalMetrics,
                    bottomObstructionPx = bottomInsetPx + chromeInteractiveHeightPx,
                    leftObstructionPx = navigationLeftInsetPx,
                    rightObstructionPx = navigationRightInsetPx,
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                    emptyMessage = "Opening terminal…",
                    onViewportSizeChanged = onViewportSizeChanged,
                    onResize = onResize,
                    resizeEnabled = resumed && state.focus == FocusOwner.Self,
                    onRequestKeyboard = onRequestKeyboard,
                )
            }
            Column(
                Modifier.align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .testTag("terminal-bottom-chrome")
                    .windowInsetsPadding(bottomInsets),
            ) {
                Column(Modifier.onSizeChanged { chromeInteractiveHeightPx = it.height }) {
                    if (screen != null && composerVisible) {
                        Column(
                            Modifier.fillMaxWidth()
                                .testTag("terminal-composer-overlay")
                                .padding(horizontal = 8.dp, vertical = 6.dp),
                        ) {
                            TerminalAttachmentStrip(
                                draft = attachments,
                                onRemove = { imageId ->
                                    val removed = activeDraftStore.transitionAttachments(screen.tabId) {
                                        it.remove(imageId)
                                    }
                                    removed.removed.forEach { it.image.file.delete() }
                                },
                            )
                            TerminalInputBar(
                                value = composer.value,
                                onValueChange = { next ->
                                    activeDraftStore.updateComposer(screen.tabId) {
                                        it.updateValue(next).state
                                    }
                                },
                                onSend = ::submitComposer,
                                onAddImage = { showImageSources = true },
                                addImageEnabled = state.focus == FocusOwner.Self &&
                                    !attachments.preparing && !attachments.submitting,
                                focusRequester = inputFocus,
                                enabled = state.focus == FocusOwner.Self && !attachments.submitting,
                            )
                        }
                    }
                    if (state.showTakeFocus && screen != null) {
                        Button(
                            onClick = { onTakeFocus(cols, rows) },
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                        ) { Text("Take Focus") }
                    }
                    ExtraKeys(
                        screen = screen,
                        scrollback = scrollback,
                        expanded = keyBarExpanded,
                        onExpandedChange = onKeyBarExpandedChange,
                        onInput = onInput,
                        onOpenComposer = {
                            screen?.tabId?.let { tabId ->
                                activeDraftStore.updateComposer(tabId) { it.open() }
                            }
                        },
                        submitComposerDraft = composerVisible &&
                            (composer.value.text.isNotEmpty() || attachments.items.isNotEmpty() ||
                                attachments.preparing),
                        submissionEnabled = !attachments.preparing && !attachments.submitting,
                        onSubmitComposer = ::submitComposer,
                    )
                }
            }
        }
    }

    if (showImageSources) {
        AlertDialog(
            onDismissRequest = { showImageSources = false },
            title = { Text("Attach image") },
            text = { Text("Choose a source") },
            confirmButton = {
                TextButton(
                    onClick = {
                        showImageSources = false
                        val tabId = screen?.tabId ?: return@TextButton
                        val remaining = TerminalAttachmentDraft.MAX_IMAGES -
                            activeDraftStore.draftFor(tabId).attachments.items.size
                        if (remaining <= 0) {
                            setAttachmentMessage(tabId, "You can attach up to 4 images.")
                        } else {
                            picker.launch(TerminalImageSource.Gallery, remaining, tabId) {
                                handlePickerResult(tabId, it)
                            }
                        }
                    },
                    modifier = Modifier.testTag("terminal-image-source-gallery"),
                ) { Text("Gallery") }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        showImageSources = false
                        val tabId = screen?.tabId ?: return@TextButton
                        val remaining = TerminalAttachmentDraft.MAX_IMAGES -
                            activeDraftStore.draftFor(tabId).attachments.items.size
                        if (remaining <= 0) {
                            setAttachmentMessage(tabId, "You can attach up to 4 images.")
                        } else {
                            picker.launch(TerminalImageSource.Camera, remaining, tabId) {
                                handlePickerResult(tabId, it)
                            }
                        }
                    },
                    modifier = Modifier.testTag("terminal-image-source-camera"),
                ) { Text("Camera") }
            },
        )
    }

    if (showDiscardDrafts) {
        val draftWorkInProgress = allDrafts.values.any {
            it.attachments.preparing || it.attachments.submitting
        }
        AlertDialog(
            onDismissRequest = { showDiscardDrafts = false },
            modifier = Modifier.testTag("terminal-draft-discard-dialog"),
            title = { Text("Leave remote terminal?") },
            text = {
                Text(
                    if (draftWorkInProgress) {
                        "Wait for image preparation or upload to finish before leaving."
                    } else {
                        "Draft text and attached images are still on this phone."
                    },
                )
            },
            confirmButton = {
                Button(enabled = !draftWorkInProgress, onClick = {
                    val removed = activeDraftStore.discardAll()
                    if (!activeDraftStore.hasDrafts()) {
                        removed.forEach { it.image.file.delete() }
                        showDiscardDrafts = false
                        onBack()
                    }
                }) { Text("Discard drafts and leave") }
            },
            dismissButton = {
                TextButton(onClick = { showDiscardDrafts = false }) { Text("Keep editing") }
            },
        )
    }

}

@Composable
private fun TerminalViewport(
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow>,
    scrollbackLoading: Boolean,
    scrollbackHasMore: Boolean,
    scrollbackError: String?,
    onLoadScrollback: () -> Unit,
    onRestartScrollback: () -> Unit,
    metrics: TerminalMetrics,
    bottomObstructionPx: Int,
    leftObstructionPx: Int,
    rightObstructionPx: Int,
    modifier: Modifier,
    emptyMessage: String,
    onViewportSizeChanged: (TerminalSize) -> Unit,
    onResize: (Int, Int) -> Unit,
    resizeEnabled: Boolean,
    onRequestKeyboard: () -> Unit,
) {
    val density = LocalDensity.current
    val horizontalPaddingPx = with(density) { 4.dp.roundToPx() }
    val verticalPaddingPx = with(density) { 3.dp.roundToPx() }
    BoxWithConstraints(
        modifier.background(Color(0xFF07111B)),
    ) {
        val measuredSize = terminalViewportSizePx(
            viewportWidthPx = constraints.maxWidth,
            viewportHeightPx = constraints.maxHeight,
            leftObstructionPx = leftObstructionPx,
            rightObstructionPx = rightObstructionPx,
            bottomObstructionPx = bottomObstructionPx,
            horizontalPaddingPx = horizontalPaddingPx,
            verticalPaddingPx = verticalPaddingPx,
            cellWidthPx = metrics.cellWidthPx,
            lineHeightPx = metrics.lineHeightPx,
        )
        val currentMeasuredSize by rememberUpdatedState(measuredSize)
        val currentOnResize by rememberUpdatedState(onResize)
        LaunchedEffect(measuredSize) {
            onViewportSizeChanged(measuredSize)
        }
        LaunchedEffect(screen?.tabId, resizeEnabled) {
            if (screen != null && resizeEnabled) {
                snapshotFlow { currentMeasuredSize }
                    .settledTerminalSizes()
                    .collect { size -> currentOnResize(size.cols, size.rows) }
            }
        }
        Box(
            Modifier.fillMaxSize().absolutePadding(
                left = with(density) { (leftObstructionPx + horizontalPaddingPx).toDp() },
                top = with(density) { verticalPaddingPx.toDp() },
                right = with(density) { (rightObstructionPx + horizontalPaddingPx).toDp() },
                bottom = with(density) { verticalPaddingPx.toDp() },
            ),
        ) {
            key(screen?.tabId) {
                TerminalGrid(
                    screen = screen,
                    scrollback = scrollback,
                    scrollbackLoading = scrollbackLoading,
                    scrollbackHasMore = scrollbackHasMore,
                    scrollbackError = scrollbackError,
                    onLoadScrollback = onLoadScrollback,
                    onRestartScrollback = onRestartScrollback,
                    modifier = Modifier.fillMaxSize(),
                    onRequestKeyboard = onRequestKeyboard,
                    metrics = metrics,
                )
            }
            if (screen == null) {
                Text(
                    emptyMessage,
                    modifier = Modifier.align(Alignment.Center),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun ConnectionRail(state: RemoteClientState, onReconnect: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth()
            .background(state.connection.color().copy(alpha = 0.14f))
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val path = state.connectedEndpoint?.path
        val label = if (
            state.connection == ConnectionState.Connected &&
            path != null && path != com.adroited.aiterm.remote.RemotePath.UNKNOWN
        ) {
            "${state.connection.label()} · ${path.name}"
        } else {
            state.connection.label()
        }
        Text(label, style = MaterialTheme.typography.labelMedium)
        state.lastError?.let {
            Text("  $it", modifier = Modifier.weight(1f), maxLines = 1)
        } ?: Spacer(Modifier.weight(1f))
        if (state.connection == ConnectionState.Disconnected) {
            TextButton(onClick = onReconnect) { Text("Reconnect") }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TerminalGrid(
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow>,
    scrollbackLoading: Boolean,
    scrollbackHasMore: Boolean,
    scrollbackError: String?,
    onLoadScrollback: () -> Unit,
    onRestartScrollback: () -> Unit,
    modifier: Modifier,
    onRequestKeyboard: () -> Unit,
    metrics: TerminalMetrics,
) {
    val terminalRows = remember(scrollback, screen?.visible) {
        scrollback.asReversed() + (screen?.visible ?: emptyList())
    }
    val listState = rememberLazyListState(
        initialFirstVisibleItemIndex = scrollback.size.coerceAtMost(terminalRows.lastIndex.coerceAtLeast(0)),
    )
    val loading by rememberUpdatedState(scrollbackLoading)
    val hasMore by rememberUpdatedState(scrollbackHasMore && screen != null)
    val loadOlder by rememberUpdatedState(onLoadScrollback)
    val restartHistory by rememberUpdatedState(onRestartScrollback)
    val historySize by rememberUpdatedState(scrollback.size)
    val revision by rememberUpdatedState(screen?.revision)
    // Cached history can outlive this screen while the API view is open. Start
    // a fresh browse on the next drag from live output when returning to it.
    var lastBrowseRevision by remember {
        mutableStateOf(if (scrollback.isNotEmpty() || !scrollbackHasMore) null else screen?.revision)
    }
    var requestedDuringDrag by remember { mutableStateOf(false) }
    var dragStartedAtLiveOutput by remember { mutableStateOf(false) }
    LaunchedEffect(listState) {
        listState.interactionSource.interactions.collect { interaction ->
            if (interaction is DragInteraction.Start) {
                requestedDuringDrag = false
                // A terminal shorter than the drawable area cannot scroll its
                // first live row all the way to the top; its tail is live too.
                dragStartedAtLiveOutput = listState.firstVisibleItemIndex >= historySize ||
                    !listState.canScrollForward
            }
        }
    }
    val historyScroll = remember(listState) {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                // Only a deliberate drag toward older output requests a page. A page
                // insertion, live update, fling, or recomposition never starts a request.
                if (source == NestedScrollSource.UserInput && available.y > 0f &&
                    !requestedDuringDrag && !loading && revision != null
                ) {
                    if (dragStartedAtLiveOutput && revision != lastBrowseRevision) {
                        requestedDuringDrag = true
                        lastBrowseRevision = revision
                        restartHistory()
                    } else if (listState.firstVisibleItemIndex <= 6 && hasMore) {
                        requestedDuringDrag = true
                        lastBrowseRevision = revision
                        loadOlder()
                    }
                }
                return Offset.Zero
            }
        }
    }
    val density = LocalDensity.current
    Box(
        modifier.clickable(onClick = onRequestKeyboard).testTag("terminal-grid"),
    ) {
        SelectionContainer {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize()
                    .nestedScroll(historyScroll)
                    .then(if (WindowInsets.ime.getBottom(density) > 0) Modifier.imeNestedScroll() else Modifier)
                    .testTag("terminal-render-content"),
            ) {
                itemsIndexed(
                    terminalRows,
                    key = { index, _ ->
                        val tabId = screen?.tabId ?: "none"
                        if (index < scrollback.size) {
                            "$tabId:history:${scrollback.size - index - 1}"
                        } else {
                            "$tabId:live:${index - scrollback.size}"
                        }
                    },
                ) { index, row ->
                    TerminalRowGrid(row, index, metrics)
                }
            }
        }
        if (scrollbackLoading || scrollbackError != null) {
            Surface(
                modifier = Modifier.align(Alignment.TopCenter).padding(8.dp),
                shape = MaterialTheme.shapes.small,
                color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f),
            ) {
                Text(
                    if (scrollbackLoading) "Loading history…" else "Couldn’t load history. Scroll back to retry.",
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                    style = MaterialTheme.typography.labelMedium,
                )
            }
        }
        val cursor = screen?.cursor
        val cursorIndex = cursor?.let { scrollback.size + it.row }
        val cursorItem = cursorIndex?.let { wanted ->
            listState.layoutInfo.visibleItemsInfo.firstOrNull { it.index == wanted }
        }
        if (cursor != null && cursor.visible && cursorItem != null) {
            val color = Color(0xFF63D3E1)
            Box(
                Modifier
                    .offset(
                        x = metrics.cellWidth * cursor.col,
                        y = with(density) { cursorItem.offset.toDp() },
                    )
                    .size(metrics.cellWidth, metrics.lineHeight)
                    .testTag("terminal-cursor")
                    .drawBehind {
                        val thickness = 2.dp.toPx()
                        when (cursor.shape) {
                            CursorShape.Block -> drawRect(color.copy(alpha = 0.35f))
                            CursorShape.Beam -> drawRect(color, size = androidx.compose.ui.geometry.Size(thickness, size.height))
                            CursorShape.Underline -> drawRect(
                                color,
                                topLeft = androidx.compose.ui.geometry.Offset(0f, size.height - thickness),
                                size = androidx.compose.ui.geometry.Size(size.width, thickness),
                            )
                        }
                    },
            )
        }
    }
}

@Composable
private fun TerminalInputBar(
    value: TextFieldValue,
    onValueChange: (TextFieldValue) -> Unit,
    onSend: () -> Unit,
    onAddImage: () -> Unit,
    addImageEnabled: Boolean,
    focusRequester: FocusRequester,
    enabled: Boolean,
) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.medium,
        color = Color(0xFF0B1A26),
        tonalElevation = 4.dp,
        shadowElevation = 8.dp,
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            TextButton(
                onClick = onAddImage,
                enabled = addImageEnabled,
                modifier = Modifier.size(48.dp)
                    .semantics { contentDescription = "Attach image" }
                    .testTag("terminal-add-image"),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp),
            ) {
                Text("＋", color = Color(0xFF63D3E1), fontSize = 22.sp)
            }
            BasicTextField(
                value = value,
                onValueChange = onValueChange,
                modifier = Modifier.weight(1f).height(44.dp)
                    .focusRequester(focusRequester)
                    .testTag("terminal-composer"),
                enabled = enabled,
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium.copy(
                    color = Color(0xFFD8E6EF),
                    fontFamily = FontFamily.Monospace,
                ),
                cursorBrush = SolidColor(Color(0xFF63D3E1)),
                keyboardOptions = KeyboardOptions(
                    capitalization = KeyboardCapitalization.Sentences,
                    autoCorrectEnabled = true,
                    keyboardType = KeyboardType.Text,
                    imeAction = ImeAction.Go,
                ),
                keyboardActions = KeyboardActions(onGo = { onSend() }),
                decorationBox = { inner ->
                    Box(
                        Modifier.fillMaxSize()
                            .border(1.dp, Color(0xFF315269), MaterialTheme.shapes.small)
                            .background(Color(0xFF07111B), MaterialTheme.shapes.small)
                            .padding(horizontal = 12.dp),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        if (value.text.isEmpty()) {
                            Text(
                                if (!enabled) "Take focus to type" else "Type a command or prompt…",
                                color = Color(0xFF6F8798),
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                        inner()
                    }
                },
            )
        }
    }
}

@Composable
private fun TerminalRowGrid(row: ScreenRow, rowIndex: Int, metrics: TerminalMetrics) {
    val uriHandler = LocalUriHandler.current
    val plain = row.plainText()
    val links = SAFE_LINK.findAll(plain).mapNotNull { match ->
        val candidate = match.value.trimEnd('.', ',', ')', ']', '}')
        candidate.takeIf { it.length <= 2_048 && isSafeRemoteLink(it) }?.let {
            match.range.first until (match.range.first + candidate.length) to candidate
        }
    }.toList()
    var textOffset = 0
    Row(
        Modifier.height(metrics.lineHeight).testTag("terminal-row")
            .semantics(mergeDescendants = true) { text = AnnotatedString(plain) },
    ) {
        row.cells.forEachIndexed { cellIndex, cell ->
            if (cell.continuation) return@forEachIndexed
            val cellRange = textOffset until (textOffset + cell.text.length)
            val link = links.firstOrNull { (range, _) ->
                cellRange.first < range.last + 1 && range.first < cellRange.last + 1
            }?.second
            textOffset += cell.text.length
            val slotBackground = when {
                cell.attributes.inverse -> cell.foreground.color(Color(0xFFD8E6EF))
                else -> cell.background.color(Color.Transparent)
            }
            var slot = Modifier.width(metrics.cellWidth * cell.width).height(metrics.lineHeight)
                .background(slotBackground).testTag("terminal-cell-$rowIndex-$cellIndex")
            if (link != null) slot = slot.clickable { uriHandler.openUri(link) }
            Box(slot) {
                TerminalCellText(cell, linked = link != null, metrics = metrics)
            }
        }
    }
}

@Composable
private fun TerminalCellText(cell: ScreenCell, linked: Boolean, metrics: TerminalMetrics) {
    val text = buildAnnotatedString {
        val foreground = cell.foreground.color(default = Color(0xFFD8E6EF))
        val background = cell.background.color(default = Color.Transparent)
        val effectiveForeground = when {
            cell.attributes.hidden -> Color.Transparent
            cell.attributes.inverse -> background.ifTransparent(Color(0xFF07111B))
            cell.attributes.faint -> foreground.copy(alpha = 0.58f)
            else -> foreground
        }
        val effectiveBackground = when {
            else -> Color.Transparent
        }
        val decoration = when {
            linked && cell.attributes.strikethrough ->
                TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
            linked -> TextDecoration.Underline
            else -> null
        }
        pushStyle(cell.attributes.span(effectiveForeground, effectiveBackground).let { style ->
            if (decoration == null) style else style.copy(textDecoration = decoration)
        })
        append(cell.text)
        pop()
    }
    BasicText(
        text = text,
        style = metrics.textStyle,
        maxLines = 1,
        softWrap = false,
    )
}

private data class TerminalMetrics(
    val cellWidth: Dp,
    val lineHeight: Dp,
    val cellWidthPx: Int,
    val lineHeightPx: Int,
    val textStyle: TextStyle,
)

@Composable
private fun rememberTerminalMetrics(): TerminalMetrics {
    val density = LocalDensity.current
    val measurer = rememberTextMeasurer()
    val textStyle = remember(density.density, density.fontScale) {
        TextStyle(
            color = Color(0xFFD8E6EF),
            fontFamily = FontFamily.Monospace,
            fontSize = 13.sp,
            lineHeight = 16.sp,
        )
    }
    val measured = measurer.measure(
        text = AnnotatedString("M"),
        style = textStyle,
        maxLines = 1,
        softWrap = false,
    )
    return TerminalMetrics(
        cellWidth = with(density) { measured.size.width.toDp() },
        lineHeight = with(density) { measured.size.height.toDp() },
        cellWidthPx = measured.size.width,
        lineHeightPx = measured.size.height,
        textStyle = textStyle,
    )
}

@Composable
private fun ExtraKeys(
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow>,
    expanded: Boolean,
    onExpandedChange: (Boolean) -> Unit,
    onInput: (String) -> Unit,
    onOpenComposer: () -> Unit,
    submitComposerDraft: Boolean,
    submissionEnabled: Boolean,
    onSubmitComposer: () -> Unit,
) {
    var control by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    val applicationCursor = screen?.modes?.applicationCursor == true
    fun send(value: String) {
        var output = value
        if (control && output.length == 1) {
            val code = output[0].uppercaseChar().code
            if (code in 64..95) output = (code and 0x1f).toChar().toString()
        }
        if (alt) output = "\u001b$output"
        onInput(output)
        control = false
        alt = false
    }
    if (expanded) {
        Row(
            Modifier.fillMaxWidth()
                .background(MaterialTheme.colorScheme.surfaceVariant)
                .padding(horizontal = 4.dp, vertical = 3.dp)
                .testTag("extra-keys"),
            horizontalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            Row(
                modifier = Modifier.weight(1f).horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                ExtraKey("Type", action = onOpenComposer)
                ExtraKey("Esc") { send("\u001b") }
                ExtraKey(if (control) "Ctrl ●" else "Ctrl") { control = !control }
                ExtraKey(if (alt) "Alt ●" else "Alt") { alt = !alt }
                ExtraKey("Tab") { send("\t") }
                ExtraKey(
                    "Enter",
                    modifier = Modifier.testTag("terminal-enter"),
                    enabled = !submitComposerDraft || submissionEnabled,
                ) {
                    if (submitComposerDraft) {
                        onSubmitComposer()
                        control = false
                        alt = false
                    } else {
                        send("\r")
                    }
                }
                ExtraKey("⌫") { send("\u007f") }
                ExtraKey("←") { send(if (applicationCursor) "\u001bOD" else "\u001b[D") }
                ExtraKey("↑") { send(if (applicationCursor) "\u001bOA" else "\u001b[A") }
                ExtraKey("↓") { send(if (applicationCursor) "\u001bOB" else "\u001b[B") }
                ExtraKey("→") { send(if (applicationCursor) "\u001bOC" else "\u001b[C") }
                ExtraKey("PgUp") { send("\u001b[5~") }
                ExtraKey("PgDn") { send("\u001b[6~") }
                ExtraKey("|") { send("|") }
                ExtraKey("/") { send("/") }
                ExtraKey("~") { send("~") }
                ExtraKey("Paste") {
                    clipboard.getText()?.text?.let { text ->
                        send(if (screen?.modes?.bracketedPaste == true) "\u001b[200~$text\u001b[201~" else text)
                    }
                }
                ExtraKey("Copy screen") {
                    val text = (scrollback.asReversed() + (screen?.visible ?: emptyList()))
                        .joinToString("\n", transform = ScreenRow::plainText)
                    clipboard.setText(AnnotatedString(text))
                }
            }
            ExtraKey(
                label = "⌄",
                action = { onExpandedChange(false) },
                modifier = Modifier.testTag("collapse-extra-keys"),
            )
        }
    } else {
        Box(
            modifier = Modifier.fillMaxWidth()
                .height(28.dp)
                .background(MaterialTheme.colorScheme.surfaceVariant)
                .clickable { onExpandedChange(true) }
                .semantics { contentDescription = "Show terminal keys" }
                .testTag("expand-extra-keys"),
            contentAlignment = Alignment.Center,
        ) {
            Text("⌃")
        }
    }
}

@Composable
private fun ExtraKey(
    label: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    action: () -> Unit,
) {
    OutlinedButton(onClick = action, enabled = enabled, modifier = modifier.height(38.dp)) { Text(label) }
}

private fun CellAttributes.span(foreground: Color, background: Color) = SpanStyle(
    color = foreground,
    background = background,
    fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
    fontStyle = if (italic) FontStyle.Italic else FontStyle.Normal,
    textDecoration = when {
        underline && strikethrough -> TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
        underline -> TextDecoration.Underline
        strikethrough -> TextDecoration.LineThrough
        else -> null
    },
)

private fun TerminalColor.color(default: Color): Color = when (this) {
    TerminalColor.Default -> default
    is TerminalColor.Rgb -> Color(red, green, blue)
    is TerminalColor.Indexed -> terminalIndexedColor(index)
}

internal fun terminalIndexedColor(index: Int): Color {
    val value = index.coerceIn(0, 255)
    if (value < 16) return TERMINAL_PALETTE[value]
    if (value < 232) {
        val cube = value - 16
        val levels = intArrayOf(0, 95, 135, 175, 215, 255)
        return Color(levels[cube / 36], levels[(cube / 6) % 6], levels[cube % 6])
    }
    val gray = 8 + (value - 232) * 10
    return Color(gray, gray, gray)
}

private fun Color.ifTransparent(fallback: Color): Color = if (alpha == 0f) fallback else this

internal fun terminalImageErrorMessage(error: Throwable): String = when (error) {
    is TerminalImageNormalizationError -> when (error.code) {
        TerminalImageNormalizationError.Code.INPUT_TOO_LARGE,
        TerminalImageNormalizationError.Code.OUTPUT_TOO_LARGE ->
            "This image is too large. Choose a smaller image."
        TerminalImageNormalizationError.Code.CONTENT_UNAVAILABLE ->
            "The image could not be opened. Choose it again."
        else -> "The image could not be prepared. Choose a different image."
    }
    else -> "The image could not be prepared. Choose a different image."
}

private fun terminalUploadErrorMessage(error: Throwable): String = when {
    error is RemoteUploadException && error.code in setOf(
        "remote.unsupported",
        "protocol.unknown_request",
    ) ->
        "Update AITerm on the desktop to attach images."
    !error.message.isNullOrBlank() -> error.message!!
    else -> "The image upload failed. Check the connection and try again."
}

private fun ConnectionState.label(): String = when (this) {
    ConnectionState.Disconnected -> "DISCONNECTED"
    ConnectionState.Connecting -> "CONNECTING"
    ConnectionState.Connected -> "CONNECTED"
    ConnectionState.Reconnecting -> "RECONNECTING"
    ConnectionState.Locked -> "LOCKED"
    ConnectionState.Revoked -> "ACCESS REVOKED"
}

@Composable
private fun ConnectionState.color(): Color = when (this) {
    ConnectionState.Connected -> MaterialTheme.colorScheme.tertiary
    ConnectionState.Connecting, ConnectionState.Reconnecting -> MaterialTheme.colorScheme.primary
    ConnectionState.Disconnected, ConnectionState.Locked, ConnectionState.Revoked -> MaterialTheme.colorScheme.error
}

private val TERMINAL_PALETTE = listOf(
    Color(0xFF07111B), Color(0xFFC94F56), Color(0xFF54B399), Color(0xFFD6A84B),
    Color(0xFF5C91D9), Color(0xFFB677D0), Color(0xFF52B8C8), Color(0xFFD8E6EF),
    Color(0xFF536575), Color(0xFFFF7378), Color(0xFF70D9B7), Color(0xFFFFCC66),
    Color(0xFF79AFFF), Color(0xFFD892EA), Color(0xFF74D9EA), Color(0xFFFFFFFF),
)

private val SAFE_LINK = Regex("https?://[^\\s<>{}\\[\\]\\\"']+")

internal fun isSafeRemoteLink(candidate: String): Boolean = try {
    val uri = URI(candidate)
    uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrBlank() && uri.userInfo == null
} catch (_: Exception) {
    false
}
