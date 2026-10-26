# A view of one `Dict` value survives a lookup of another: `d["a"]` and
# `d["b"]` lend the same `value` interior, and a lookup replaces nothing,
# so `v` is still readable after `u` is made, as at the pin.
def main() raises:
    var d: Dict[String, String] = {}
    d["a"] = String("x  ")
    d["b"] = String("y ")
    var v = d["a"].rstrip()
    var u = d["b"].rstrip()
    print(String(v), String(u))
