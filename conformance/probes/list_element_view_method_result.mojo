# Question: a view returned by a method on a `List` element place,
# `var v = ys[0].rstrip()`, read after the statement.
# Upstream (`1.1.0.dev2026082605`) prints `q`.
#
# Mojito's VM fails with "use after Pointer deallocation": the element
# receiver is an `Index` place, which the view-result borrow neither lends
# nor materializes, so nothing keeps the element's bytes alive for the view.
#
# On the fix: promote this file to `assets/ok/list_element_view_method_result.mojo`
# with its manifest rows, and delete the matching roadmap checkbox.
def main():
    var ys: List[String] = [String("q  ")]
    var v = ys[0].rstrip()
    var z = String(v)
    print(z)
