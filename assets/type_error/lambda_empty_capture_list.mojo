# An explicit `{}` capture list captures nothing: a free variable in the body
# is an error rather than an implicit imm capture.
# expect: could not infer capture convention for 'z'
def main():
    var z = 10
    var f: def(x: Int) -> Int = lambda (x: Int) {} -> Int: x + z
    print(f(5))
