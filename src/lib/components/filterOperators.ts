export const OPERAND_REQUIRED_OPS = ["<", ">", ">=", "<=", "="] as const;

export const INCOMPLETE_OPS = new RegExp(`^(${OPERAND_REQUIRED_OPS.join("|")})$`);
