# The `ref`-field spelling of the ordinary fixture of the same name (a kept
# Mojito extension, see docs/non-goals.md); the origin slot is bound exactly
# as there.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def main():
    var local: List[Int] = [9]
    ref view = local
    var sink = List[RefBox[origin_of(view)]]()
    sink.append(RefBox(view))
    print(sink[0].value[0])
