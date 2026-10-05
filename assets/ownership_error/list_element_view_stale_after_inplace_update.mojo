# expect: invalidated interior reference
# An in-place update of a List element writes the element through its
# reference, so a view lent from that element's bytes is stale afterwards.
# requires: stdlib
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    ys[0] += "tail"
    print(v)
