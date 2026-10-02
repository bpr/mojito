# PROBE (divergence): a view of one `Dict` value is staled by a second lookup.
# Upstream (`1.2.0.dev2026092105`) prints `x y`.
#
# Mojito rejects with "use of invalidated interior reference 'v' to
# 'd["value"]~'": a `Dict` lookup defines a fresh `value` generation, and the
# view `d["a"].rstrip()` lends the whole subtree below the earlier one, so the
# second lookup stales it. Upstream's view names `d["value"]["bytes"]`, which
# a lookup does not replace.
#
# On the fix: promote this file to `assets/ok/` with its manifest rows, and
# delete the matching roadmap checkbox.
def main() raises:
    var d: Dict[String, String] = {}
    d["a"] = String("x  ")
    d["b"] = String("y ")
    var v = d["a"].rstrip()
    var u = d["b"].rstrip()
    print(String(v), String(u))
