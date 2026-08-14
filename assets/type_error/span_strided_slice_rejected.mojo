# expect: no overload matches
# Spans have no strided slicing: only the strict contiguous overload
# exists, so a stride selects no `__getitem__`.
def main():
    var xs: List[Int] = [1, 2, 3]
    var sp = Span(xs)
    print(sp[::-1])
