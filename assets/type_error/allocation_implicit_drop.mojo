# Allocation is linear (Deinitable where False): letting one fall out of
# scope without dealloc/leak abandons its explicit-destroy obligation.
# expect: release it with dealloc(allocation^)
from std.memory import Layout

def main():
    var allocation = alloc(Layout[Int](count=1))
    print(1)
