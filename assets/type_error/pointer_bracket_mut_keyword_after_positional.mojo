# `mut` is infer-only, so its keyword binds only ahead of every positional.
# expect: unexpected keyword parameter 'mut'
def main():
    var x = 5
    var p = Pointer(to=x)
    var r: Pointer[Int, mut=True, origin_of(x)] = p
    print(r[])
