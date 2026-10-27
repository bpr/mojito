# expect: expression does not designate a value with an origin
# A loop binder not declared `var` binds each element by reference, so it
# is no tracked root a `^` may move out of.
def main():
    var xs = [String("a"), String("b")]
    for x in xs:
        var y = x^
        print(y)
