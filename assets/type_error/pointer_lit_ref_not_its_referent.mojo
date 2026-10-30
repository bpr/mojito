# A `!lit.ref` value is not its referent: no read goes through it.
# expect: expected Int, found ref[
def main():
    var x = 7
    var r = __get_mvalue_as_litref(x)
    var w: Int = r
    print(w)
