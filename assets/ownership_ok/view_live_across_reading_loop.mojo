# A value loop over a list lends its elements shared, so it coexists with a
# live whole-place shared loan of the same list (a borrowed view); only a
# `for ref` loop's mutable element loan is exclusive against the view.
def borrow[origin: Origin[mut=False]](ref[origin] values: List[Int]) -> ref[origin] List[Int]:
    return values

def main():
    var values: List[Int] = [1, 2, 3]
    ref view = borrow(values)
    var total = 0
    for x in values:
        total += x
    print(total, view[0])
