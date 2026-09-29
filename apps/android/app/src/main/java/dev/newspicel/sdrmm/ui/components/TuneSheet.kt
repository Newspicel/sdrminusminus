package dev.newspicel.sdrmm.ui.components

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors

object Tuning {
    private const val MAX_HZ = 1e11
    private const val HZ_PER_MHZ = 1e6

    fun hz(megahertz: String): Double? {
        val value = megahertz.trim().replace(',', '.').toDoubleOrNull() ?: return null
        val hz = value * HZ_PER_MHZ
        return if (hz.isFinite() && hz > 0 && hz < MAX_HZ) hz else null
    }
}

@Composable
fun TuneSheet(
    onSet: (String) -> Int?,
    onDismiss: () -> Unit,
) {
    var text by rememberSaveable { mutableStateOf("") }

    @StringRes var error by rememberSaveable { mutableStateOf<Int?>(null) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.tune)) },
        text = {
            Column {
                OutlinedTextField(
                    value = text,
                    onValueChange = {
                        text = it
                        error = null
                    },
                    label = { Text(stringResource(R.string.tune_mhz)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
                    isError = error != null,
                )
                error?.let { Text(stringResource(it), color = LocalStatusColors.current.danger) }
            }
        },
        confirmButton = { TextButton(onClick = { error = onSet(text) }) { Text(stringResource(R.string.tune_set)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
