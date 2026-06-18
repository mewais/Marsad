package com.mewais.marsad

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

// M02 — Android app entry point.
//
// TODO(M02 code-agent): build the SSH-only Compose UI over the UniFFI-generated
// marsad-core bindings, faithful to
// _extracted/design_handoff_marsad/Marsad Mobile.dc.html: bottom tabs + FAB,
// session detail tabs, gate BOTTOM SHEET, unified (stacked) diffs, terminal accessory
// keypad with sticky CTRL/ALT, FCM glue. See apps/android/AGENTS.md and docs/specs/core.md.

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MarsadRoot() }
    }
}

@Composable
fun MarsadRoot() {
    Column(modifier = Modifier.padding(16.dp)) {
        Text("Marsad — مرصد")
        Text("Scaffolding. UI not yet implemented — see apps/android/AGENTS.md.")
    }
}
