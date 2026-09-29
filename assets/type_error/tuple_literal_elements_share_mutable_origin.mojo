# expect: aliasing values passed mutably to 'args' argument and passed mutably to 'args' argument in 'Tuple[
# A tuple literal is its `Tuple` initializer's call over `var *args`: two
# spans over one mutable list reach it mutably through that call, as at the
# pin. (The same literal over a read parameter's list is accepted.)
def main():
    var xs: List[Int] = [4, 5, 6]
    var t = (Span(xs), Span(xs))
    print(len(t[0]))
