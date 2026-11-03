# The removed `capturing` effect rejects in a local's annotation: a
# closure's value is its storage struct, held in an unannotated local.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def main():
    var k = 10
    var f: def(Int) capturing[_] -> Int = lambda (x: Int) {k} -> Int: x + k
    print(f(1))
