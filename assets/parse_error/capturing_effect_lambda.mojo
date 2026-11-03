# The removed `capturing` effect rejects on a lambda: it lists its
# captures instead.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def main():
    var k = 1
    var f = lambda (x: Int) capturing -> Int: x + k
    print(f(1))
