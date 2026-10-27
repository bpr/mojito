# expect: expression does not designate a value with an origin
# A `ref` binding does not own its referent: it reaches no tracked root a
# `^` may move out of, as upstream's `emitTransfer` decides.
def main():
    var s = String("a")
    ref r = s
    var t = r^
    print(t)
