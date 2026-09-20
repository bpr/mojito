# Compile-time `//` and `%` floor toward the divisor's sign on machine `Int`
# as on exact literals. Every evaluator shares one folder
# (`mojito_types::param_expr::fold`).
def main():
    comptime a = -7 // 3
    comptime b = 7 // -3
    comptime c = -7 % 3
    comptime d = 7 % -3
    comptime e = Int(-7) // Int(3)
    comptime f = Int(7) // Int(-3)
    comptime g = Int(-7) % Int(3)
    comptime h = Int(7) % Int(-3)
    print(a, b, c, d)
    print(e, f, g, h)
