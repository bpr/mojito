# expect: aliasing values passed mutably to 'args' argument and passed mutably to 'args' argument in 'Tuple[
# An explicit `Tuple(...)` initializer call may not gather two arguments that
# carry the same mutable origin, as at the pin.
def main():
    var xs: List[Int] = [4, 5, 6]
    var t = Tuple(Span(xs), Span(xs))
    print(len(t[0]))
