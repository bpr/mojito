# expect: call to 'deinit_with': no overload matches
from std.utils import Variant

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    v.deinit_with(lambda (a: Int) -> Int: a)
