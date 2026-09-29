# expect: aliasing values passed mutably to 'self' argument and passed mutably to 'value' argument in 'insert' call
# An origin a method receives only through a type argument counts in
# argument exclusivity: the receiver's `List[Span[Int, origin_of(xs)]]` and
# the inserted `Span[Int, origin_of(xs)]` both reach the mutable `xs`, as at
# the pin. `append` is accepted because upstream declares it
# `@__unsafe_nested_origins_read_only`; `insert` is not so declared.
def main():
    var xs: List[Int] = [1, 2, 3]
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    l.insert(0, Span(xs))
    print(len(l))
