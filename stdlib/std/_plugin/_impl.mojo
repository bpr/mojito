# Upstream's `std/_plugin/_impl.mojo`, verbatim.
"""Selects the active `PluginHooks` used by the stdlib.

`CurrentPlugin` is selected from the current target's opaque plugin payload.
Targets without a payload use `DefaultPlugin`, which leaves every hook at its
default value.
"""

from std.sys.info import CompilationTarget

from ._trait import PluginHooks

comptime CurrentPlugin: PluginHooks = CompilationTarget.current().stdlib_plugin
"""The active `PluginHooks`."""
