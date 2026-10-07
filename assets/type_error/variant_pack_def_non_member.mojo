# expect: Type does not exist in Variant
# A pack-keyed `def` served by its template still rejects a value of no
# alternative when the call closes its pack.
from std.utils import Variant

def first_variant[*Ts: Movable]() -> Variant[*Ts]:
    return Variant[*Ts](3.5)

def main():
    var v = first_variant[Int, String]()
    print(v.isa[Int]())
