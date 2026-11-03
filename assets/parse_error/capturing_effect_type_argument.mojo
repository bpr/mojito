# The removed `capturing` effect rejects in a function type nested in a
# local's annotation, as an `Array` element.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def main():
    var i = 2
    var fns: Array[def(x: Int) capturing[_] -> Int, 1] = [lambda (x: Int) {var i} -> Int: x * i]
    print(fns[0](3))
