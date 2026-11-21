# A bracketed `Pointer[...](to=x)` takes a place whose pointer converts to
# the bracket's instance; `x`'s origin is not `y`'s.
# expect: no matching function in initialization
def main():
    var x = 5
    var y = 6
    var p = Pointer[Int, origin_of(y)](to=x)
    print(p[], y)
