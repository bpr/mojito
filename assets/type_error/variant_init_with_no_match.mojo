# expect: 'init_with' factory for 'Variant'
# The placement constructor's factory must return one of the alternatives.
from std.utils import Variant

def make() -> Float64:
    return 1.0

def main():
    var v = Variant[Int, String](init_with=make)
    print(v.isa[Int]())
