# expect: can be given only acquire, release, acq_rel, and seq_cst orderings
# A fence orders nothing at `RELAXED`; the pin's verifier rejects it.
from std.atomic import Ordering, fence


def main():
    fence[Ordering.RELAXED]()
    print(1)
