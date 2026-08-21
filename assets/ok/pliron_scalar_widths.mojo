# Sized-integer scalar aliases: construction, wrapping +/-/* at the lane
# width, comparisons under both signednesses, and printing the lane's
# mathematical value.
def main():
    var a = UInt8(250)
    var b = UInt8(10)
    print(a + b) # wraps: 4
    print(a - b)
    print(b - a) # wraps through zero: 16
    var c = Int8(-100)
    print(c + c) # wraps: 56
    print(c * c) # wraps: 16
    print(-c)
    var e = Int16(300)
    print(e - Int16(400))
    var big = Int32(2147483647)
    print(big + Int32(1)) # wraps: -2147483648
    var u = UInt32(4294967295)
    print(u + UInt32(1)) # wraps: 0
    var i64v = Int64(-9223372036854775808)
    print(i64v)
    var u64v = UInt64(18446744073709551615)
    print(u64v)
    print(a < b)
    print(c < Int8(0))
    print(UInt8(200) > UInt8(100)) # unsigned compare
    print(Int8(-1) < Int8(1)) # signed compare
    print(a == UInt8(250))
    print(Int(a))
    print(Int(c)) # sign-extends: -100
    print(UInt(c)) # sign-extend then reinterpret
    print(Float64(a))
    print(Bool(a))
    print(Bool(UInt8(0)))
