# A `capturing[...]` callable type nested in a local's annotation, as an
# `Array` element, rejects like the bare annotation.
# expect: capturing closures cannot be materialized as runtime values (the annotation of variable 'fns')
def main():
    var i = 2
    var fns: Array[def(x: Int) capturing[_] -> Int, 1] = [lambda (x: Int) {var i} -> Int: x * i]
    print(fns[0](3))
