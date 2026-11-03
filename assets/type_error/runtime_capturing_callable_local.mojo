# A local cannot be annotated with a `capturing[...]` callable type: a
# closure's value is its storage struct, held in an unannotated local.
# expect: capturing closures cannot be materialized as runtime values (the annotation of variable 'f')
def main():
    var k = 10
    var f: def(Int) capturing[_] -> Int = lambda (x: Int) {k} -> Int: x + k
    print(f(1))
