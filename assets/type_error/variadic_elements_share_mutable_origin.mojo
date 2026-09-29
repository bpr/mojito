# expect: aliasing values passed mutably to 'args' argument and passed mutably to 'args' argument in 'show' call
# Each element a `*args` pack collects is an argument of its own: two spans
# over one mutable list reach it mutably through the same call, as at the
# pin. (The same call over a read parameter's list is accepted.)
def show[*Ts: Copyable](*args: *Ts):
    pass

def main():
    var xs: List[Int] = [4, 5, 6]
    show(Span(xs), Span(xs))
