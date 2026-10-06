package com.ezrdp.app

import android.annotation.SuppressLint
import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/** Windows virtual-key codes. */
object VK {
    const val BACK = 0x08
    const val TAB = 0x09
    const val ENTER = 0x0D
    const val SHIFT = 0x10
    const val CTRL = 0x11
    const val ALT = 0x12
    const val ESC = 0x1B
    const val PGUP = 0x21
    const val PGDN = 0x22
    const val END = 0x23
    const val HOME = 0x24
    const val LEFT = 0x25
    const val UP = 0x26
    const val RIGHT = 0x27
    const val DOWN = 0x28
    const val PRTSC = 0x2C
    const val DEL = 0x2E
    const val WIN = 0x5B
    const val F1 = 0x70
}

/**
 * Invisible view that receives the phone keyboard's typing and forwards it to the PC.
 * Suggestions/autocorrect are turned off so every key press goes straight through.
 */
@SuppressLint("ViewConstructor")
class KeyInput(context: Context, private val conn: () -> Connection?, private val onTyped: () -> Unit) : View(context) {

    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    fun show() {
        requestFocus()
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hide() {
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.hideSoftInputFromWindow(windowToken, 0)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(out: EditorInfo): InputConnection {
        out.inputType = InputType.TYPE_CLASS_TEXT or
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        out.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_ACTION_NONE
        return Ic()
    }

    private fun type(s: String) {
        if (s.isEmpty()) return
        conn()?.text(s)
        onTyped()
    }

    private fun backspace(n: Int) {
        repeat(n) { conn()?.tapKey(VK.BACK) }
        if (n > 0) onTyped()
    }

    private fun keyFor(code: Int): Int? = when (code) {
        KeyEvent.KEYCODE_DEL -> VK.BACK
        KeyEvent.KEYCODE_FORWARD_DEL -> VK.DEL
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> VK.ENTER
        KeyEvent.KEYCODE_TAB -> VK.TAB
        KeyEvent.KEYCODE_ESCAPE -> VK.ESC
        KeyEvent.KEYCODE_DPAD_LEFT -> VK.LEFT
        KeyEvent.KEYCODE_DPAD_RIGHT -> VK.RIGHT
        KeyEvent.KEYCODE_DPAD_UP -> VK.UP
        KeyEvent.KEYCODE_DPAD_DOWN -> VK.DOWN
        KeyEvent.KEYCODE_MOVE_HOME -> VK.HOME
        KeyEvent.KEYCODE_MOVE_END -> VK.END
        KeyEvent.KEYCODE_PAGE_UP -> VK.PGUP
        KeyEvent.KEYCODE_PAGE_DOWN -> VK.PGDN
        else -> null
    }

    /** Handles key events from the keyboard (and from hardware keyboards). */
    private fun handleKey(e: KeyEvent): Boolean {
        if (e.action != KeyEvent.ACTION_DOWN) return true
        val vk = keyFor(e.keyCode)
        if (vk != null) {
            conn()?.tapKey(vk)
            onTyped()
            return true
        }
        val ch = e.unicodeChar
        if (ch != 0) {
            type(String(Character.toChars(ch)))
            return true
        }
        return false
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) return super.onKeyDown(keyCode, event)
        return handleKey(event) || super.onKeyDown(keyCode, event)
    }

    private inner class Ic : BaseInputConnection(this@KeyInput, false) {
        /** Text the keyboard is still "composing" (some keyboards do this even with suggestions off). */
        private var composing = ""

        /** Changes what's on the PC from `composing` to `target` with as few key presses as possible. */
        private fun replaceComposing(target: String) {
            var common = 0
            while (common < composing.length && common < target.length && composing[common] == target[common]) common++
            backspace(composing.length - common)
            type(target.substring(common))
            composing = target
        }

        override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
            replaceComposing(text.toString())
            composing = ""
            return true
        }

        override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
            replaceComposing(text.toString())
            return true
        }

        override fun finishComposingText(): Boolean {
            composing = ""
            return true
        }

        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            if (composing.isNotEmpty()) {
                replaceComposing(composing.dropLast(beforeLength))
                return true
            }
            backspace(beforeLength)
            repeat(afterLength) { conn()?.tapKey(VK.DEL) }
            return true
        }

        override fun sendKeyEvent(event: KeyEvent): Boolean = handleKey(event)

        override fun performEditorAction(actionCode: Int): Boolean {
            conn()?.tapKey(VK.ENTER)
            onTyped()
            return true
        }
    }
}
