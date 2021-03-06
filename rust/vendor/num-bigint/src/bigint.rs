// `Add`/`Sub` ops may flip from `BigInt` to its `BigUint` magnitude
#![allow(clippy::suspicious_arithmetic_impl)]

#[cfg(any(feature = "quickcheck", feature = "arbitrary"))]
use crate::std_alloc::Box;
use crate::std_alloc::{String, Vec};
use core::cmp::Ordering::{self, Equal, Greater, Less};
#[cfg(has_try_from)]
use core::convert::TryFrom;
use core::default::Default;
use core::fmt;
use core::hash;
use core::iter::{Product, Sum};
use core::mem;
use core::ops::{
    Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div, DivAssign,
    Mul, MulAssign, Neg, Not, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub, SubAssign,
};
use core::str::{self, FromStr};
use core::{i128, u128};
use core::{i64, u64};

use num_integer::{Integer, Roots};
use num_traits::{
    CheckedAdd, CheckedDiv, CheckedMul, CheckedSub, FromPrimitive, Num, One, Pow, PrimInt, Signed,
    ToPrimitive, Zero,
};

use self::Sign::{Minus, NoSign, Plus};

use crate::big_digit::{self, BigDigit, DoubleBigDigit};
use crate::biguint;
use crate::biguint::to_str_radix_reversed;
use crate::biguint::{BigUint, IntDigits};
use crate::ParseBigIntError;
#[cfg(has_try_from)]
use crate::TryFromBigIntError;

use crate::IsizePromotion;
use crate::UsizePromotion;

/// A Sign is a `BigInt`'s composing element.
#[derive(PartialEq, PartialOrd, Eq, Ord, Copy, Clone, Debug, Hash)]
pub enum Sign {
    Minus,
    NoSign,
    Plus,
}

impl Neg for Sign {
    type Output = Sign;

    /// Negate Sign value.
    #[inline]
    fn neg(self) -> Sign {
        match self {
            Minus => Plus,
            NoSign => NoSign,
            Plus => Minus,
        }
    }
}

impl Mul<Sign> for Sign {
    type Output = Sign;

    #[inline]
    fn mul(self, other: Sign) -> Sign {
        match (self, other) {
            (NoSign, _) | (_, NoSign) => NoSign,
            (Plus, Plus) | (Minus, Minus) => Plus,
            (Plus, Minus) | (Minus, Plus) => Minus,
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Sign {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Note: do not change the serialization format, or it may break
        // forward and backward compatibility of serialized data!
        match *self {
            Sign::Minus => (-1i8).serialize(serializer),
            Sign::NoSign => 0i8.serialize(serializer),
            Sign::Plus => 1i8.serialize(serializer),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Sign {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        use serde::de::Unexpected;

        let sign: i8 = serde::Deserialize::deserialize(deserializer)?;
        match sign {
            -1 => Ok(Sign::Minus),
            0 => Ok(Sign::NoSign),
            1 => Ok(Sign::Plus),
            _ => Err(D::Error::invalid_value(
                Unexpected::Signed(sign.into()),
                &"a sign of -1, 0, or 1",
            )),
        }
    }
}

/// A big signed integer type.
#[derive(Debug)]
pub struct BigInt {
    sign: Sign,
    data: BigUint,
}

// Note: derived `Clone` doesn't specialize `clone_from`,
// but we want to keep the allocation in `data`.
impl Clone for BigInt {
    #[inline]
    fn clone(&self) -> Self {
        BigInt {
            sign: self.sign,
            data: self.data.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, other: &Self) {
        self.sign = other.sign;
        self.data.clone_from(&other.data);
    }
}

#[cfg(feature = "quickcheck")]
impl quickcheck::Arbitrary for BigInt {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        let positive = bool::arbitrary(g);
        let sign = if positive { Sign::Plus } else { Sign::Minus };
        Self::from_biguint(sign, BigUint::arbitrary(g))
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        let sign = self.sign();
        let unsigned_shrink = self.data.shrink();
        Box::new(unsigned_shrink.map(move |x| BigInt::from_biguint(sign, x)))
    }
}

#[cfg(feature = "arbitrary")]
mod abitrary_impl {
    use super::*;
    use arbitrary::{Arbitrary, Result, Unstructured};

    impl Arbitrary for BigInt {
        fn arbitrary(u: &mut Unstructured<'_>) -> Result<Self> {
            let positive = bool::arbitrary(u)?;
            let sign = if positive { Sign::Plus } else { Sign::Minus };
            Ok(Self::from_biguint(sign, BigUint::arbitrary(u)?))
        }

        fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
            let sign = self.sign();
            let unsigned_shrink = self.data.shrink();
            Box::new(unsigned_shrink.map(move |x| BigInt::from_biguint(sign, x)))
        }
    }
}

impl hash::Hash for BigInt {
    #[inline]
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        debug_assert!((self.sign != NoSign) ^ self.data.is_zero());
        self.sign.hash(state);
        if self.sign != NoSign {
            self.data.hash(state);
        }
    }
}

impl PartialEq for BigInt {
    #[inline]
    fn eq(&self, other: &BigInt) -> bool {
        debug_assert!((self.sign != NoSign) ^ self.data.is_zero());
        debug_assert!((other.sign != NoSign) ^ other.data.is_zero());
        self.sign == other.sign && (self.sign == NoSign || self.data == other.data)
    }
}

impl Eq for BigInt {}

impl PartialOrd for BigInt {
    #[inline]
    fn partial_cmp(&self, other: &BigInt) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    #[inline]
    fn cmp(&self, other: &BigInt) -> Ordering {
        debug_assert!((self.sign != NoSign) ^ self.data.is_zero());
        debug_assert!((other.sign != NoSign) ^ other.data.is_zero());
        let scmp = self.sign.cmp(&other.sign);
        if scmp != Equal {
            return scmp;
        }

        match self.sign {
            NoSign => Equal,
            Plus => self.data.cmp(&other.data),
            Minus => other.data.cmp(&self.data),
        }
    }
}

impl Default for BigInt {
    #[inline]
    fn default() -> BigInt {
        Zero::zero()
    }
}

impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.is_negative(), "", &self.data.to_str_radix(10))
    }
}

impl fmt::Binary for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.is_negative(), "0b", &self.data.to_str_radix(2))
    }
}

impl fmt::Octal for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.is_negative(), "0o", &self.data.to_str_radix(8))
    }
}

impl fmt::LowerHex for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.is_negative(), "0x", &self.data.to_str_radix(16))
    }
}

impl fmt::UpperHex for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = self.data.to_str_radix(16);
        s.make_ascii_uppercase();
        f.pad_integral(!self.is_negative(), "0x", &s)
    }
}

// Negation in two's complement.
// acc must be initialized as 1 for least-significant digit.
//
// When negating, a carry (acc == 1) means that all the digits
// considered to this point were zero. This means that if all the
// digits of a negative BigInt have been considered, carry must be
// zero as we cannot have negative zero.
//
//    01 -> ...f    ff
//    ff -> ...f    01
// 01 00 -> ...f ff 00
// 01 01 -> ...f fe ff
// 01 ff -> ...f fe 01
// ff 00 -> ...f 01 00
// ff 01 -> ...f 00 ff
// ff ff -> ...f 00 01
#[inline]
fn negate_carry(a: BigDigit, acc: &mut DoubleBigDigit) -> BigDigit {
    *acc += DoubleBigDigit::from(!a);
    let lo = *acc as BigDigit;
    *acc >>= big_digit::BITS;
    lo
}

// !-2 = !...f fe = ...0 01 = +1
// !-1 = !...f ff = ...0 00 =  0
// ! 0 = !...0 00 = ...f ff = -1
// !+1 = !...0 01 = ...f fe = -2
impl Not for BigInt {
    type Output = BigInt;

    fn not(mut self) -> BigInt {
        match self.sign {
            NoSign | Plus => {
                self.data += 1u32;
                self.sign = Minus;
            }
            Minus => {
                self.data -= 1u32;
                self.sign = if self.data.is_zero() { NoSign } else { Plus };
            }
        }
        self
    }
}

impl<'a> Not for &'a BigInt {
    type Output = BigInt;

    fn not(self) -> BigInt {
        match self.sign {
            NoSign => -BigInt::one(),
            Plus => -BigInt::from(&self.data + 1u32),
            Minus => BigInt::from(&self.data - 1u32),
        }
    }
}

// + 1 & -ff = ...0 01 & ...f 01 = ...0 01 = + 1
// +ff & - 1 = ...0 ff & ...f ff = ...0 ff = +ff
// answer is pos, has length of a
fn bitand_pos_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_b = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai &= twos_b;
    }
    debug_assert!(b.len() > a.len() || carry_b == 0);
}

// - 1 & +ff = ...f ff & ...0 ff = ...0 ff = +ff
// -ff & + 1 = ...f 01 & ...0 01 = ...0 01 = + 1
// answer is pos, has length of b
fn bitand_neg_pos(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        *ai = twos_a & bi;
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => a.truncate(b.len()),
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().cloned());
        }
    }
}

// - 1 & -ff = ...f ff & ...f 01 = ...f 01 = - ff
// -ff & - 1 = ...f 01 & ...f ff = ...f 01 = - ff
// -ff & -fe = ...f 01 & ...f 02 = ...f 00 = -100
// answer is neg, has length of longest with a possible carry
fn bitand_neg_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    let mut carry_b = 1;
    let mut carry_and = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai = negate_carry(twos_a & twos_b, &mut carry_and);
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    debug_assert!(b.len() > a.len() || carry_b == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => {
            for ai in a[b.len()..].iter_mut() {
                let twos_a = negate_carry(*ai, &mut carry_a);
                *ai = negate_carry(twos_a, &mut carry_and);
            }
            debug_assert!(carry_a == 0);
        }
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().map(|&bi| {
                let twos_b = negate_carry(bi, &mut carry_b);
                negate_carry(twos_b, &mut carry_and)
            }));
            debug_assert!(carry_b == 0);
        }
    }
    if carry_and != 0 {
        a.push(1);
    }
}

forward_val_val_binop!(impl BitAnd for BigInt, bitand);
forward_ref_val_binop!(impl BitAnd for BigInt, bitand);

// do not use forward_ref_ref_binop_commutative! for bitand so that we can
// clone as needed, avoiding over-allocation
impl<'a, 'b> BitAnd<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn bitand(self, other: &BigInt) -> BigInt {
        match (self.sign, other.sign) {
            (NoSign, _) | (_, NoSign) => BigInt::zero(),
            (Plus, Plus) => BigInt::from(&self.data & &other.data),
            (Plus, Minus) => self.clone() & other,
            (Minus, Plus) => other.clone() & self,
            (Minus, Minus) => {
                // forward to val-ref, choosing the larger to clone
                if self.len() >= other.len() {
                    self.clone() & other
                } else {
                    other.clone() & self
                }
            }
        }
    }
}

impl<'a> BitAnd<&'a BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn bitand(mut self, other: &BigInt) -> BigInt {
        self &= other;
        self
    }
}

forward_val_assign!(impl BitAndAssign for BigInt, bitand_assign);

impl<'a> BitAndAssign<&'a BigInt> for BigInt {
    fn bitand_assign(&mut self, other: &BigInt) {
        match (self.sign, other.sign) {
            (NoSign, _) => {}
            (_, NoSign) => self.set_zero(),
            (Plus, Plus) => {
                self.data &= &other.data;
                if self.data.is_zero() {
                    self.sign = NoSign;
                }
            }
            (Plus, Minus) => {
                bitand_pos_neg(self.digits_mut(), other.digits());
                self.normalize();
            }
            (Minus, Plus) => {
                bitand_neg_pos(self.digits_mut(), other.digits());
                self.sign = Plus;
                self.normalize();
            }
            (Minus, Minus) => {
                bitand_neg_neg(self.digits_mut(), other.digits());
                self.normalize();
            }
        }
    }
}

// + 1 | -ff = ...0 01 | ...f 01 = ...f 01 = -ff
// +ff | - 1 = ...0 ff | ...f ff = ...f ff = - 1
// answer is neg, has length of b
fn bitor_pos_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_b = 1;
    let mut carry_or = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai = negate_carry(*ai | twos_b, &mut carry_or);
    }
    debug_assert!(b.len() > a.len() || carry_b == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => {
            a.truncate(b.len());
        }
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().map(|&bi| {
                let twos_b = negate_carry(bi, &mut carry_b);
                negate_carry(twos_b, &mut carry_or)
            }));
            debug_assert!(carry_b == 0);
        }
    }
    // for carry_or to be non-zero, we would need twos_b == 0
    debug_assert!(carry_or == 0);
}

// - 1 | +ff = ...f ff | ...0 ff = ...f ff = - 1
// -ff | + 1 = ...f 01 | ...0 01 = ...f 01 = -ff
// answer is neg, has length of a
fn bitor_neg_pos(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    let mut carry_or = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        *ai = negate_carry(twos_a | bi, &mut carry_or);
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    if a.len() > b.len() {
        for ai in a[b.len()..].iter_mut() {
            let twos_a = negate_carry(*ai, &mut carry_a);
            *ai = negate_carry(twos_a, &mut carry_or);
        }
        debug_assert!(carry_a == 0);
    }
    // for carry_or to be non-zero, we would need twos_a == 0
    debug_assert!(carry_or == 0);
}

// - 1 | -ff = ...f ff | ...f 01 = ...f ff = -1
// -ff | - 1 = ...f 01 | ...f ff = ...f ff = -1
// answer is neg, has length of shortest
fn bitor_neg_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    let mut carry_b = 1;
    let mut carry_or = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai = negate_carry(twos_a | twos_b, &mut carry_or);
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    debug_assert!(b.len() > a.len() || carry_b == 0);
    if a.len() > b.len() {
        a.truncate(b.len());
    }
    // for carry_or to be non-zero, we would need twos_a == 0 or twos_b == 0
    debug_assert!(carry_or == 0);
}

forward_val_val_binop!(impl BitOr for BigInt, bitor);
forward_ref_val_binop!(impl BitOr for BigInt, bitor);

// do not use forward_ref_ref_binop_commutative! for bitor so that we can
// clone as needed, avoiding over-allocation
impl<'a, 'b> BitOr<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn bitor(self, other: &BigInt) -> BigInt {
        match (self.sign, other.sign) {
            (NoSign, _) => other.clone(),
            (_, NoSign) => self.clone(),
            (Plus, Plus) => BigInt::from(&self.data | &other.data),
            (Plus, Minus) => other.clone() | self,
            (Minus, Plus) => self.clone() | other,
            (Minus, Minus) => {
                // forward to val-ref, choosing the smaller to clone
                if self.len() <= other.len() {
                    self.clone() | other
                } else {
                    other.clone() | self
                }
            }
        }
    }
}

impl<'a> BitOr<&'a BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn bitor(mut self, other: &BigInt) -> BigInt {
        self |= other;
        self
    }
}

forward_val_assign!(impl BitOrAssign for BigInt, bitor_assign);

impl<'a> BitOrAssign<&'a BigInt> for BigInt {
    fn bitor_assign(&mut self, other: &BigInt) {
        match (self.sign, other.sign) {
            (_, NoSign) => {}
            (NoSign, _) => self.clone_from(other),
            (Plus, Plus) => self.data |= &other.data,
            (Plus, Minus) => {
                bitor_pos_neg(self.digits_mut(), other.digits());
                self.sign = Minus;
                self.normalize();
            }
            (Minus, Plus) => {
                bitor_neg_pos(self.digits_mut(), other.digits());
                self.normalize();
            }
            (Minus, Minus) => {
                bitor_neg_neg(self.digits_mut(), other.digits());
                self.normalize();
            }
        }
    }
}

// + 1 ^ -ff = ...0 01 ^ ...f 01 = ...f 00 = -100
// +ff ^ - 1 = ...0 ff ^ ...f ff = ...f 00 = -100
// answer is neg, has length of longest with a possible carry
fn bitxor_pos_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_b = 1;
    let mut carry_xor = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai = negate_carry(*ai ^ twos_b, &mut carry_xor);
    }
    debug_assert!(b.len() > a.len() || carry_b == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => {
            for ai in a[b.len()..].iter_mut() {
                let twos_b = !0;
                *ai = negate_carry(*ai ^ twos_b, &mut carry_xor);
            }
        }
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().map(|&bi| {
                let twos_b = negate_carry(bi, &mut carry_b);
                negate_carry(twos_b, &mut carry_xor)
            }));
            debug_assert!(carry_b == 0);
        }
    }
    if carry_xor != 0 {
        a.push(1);
    }
}

// - 1 ^ +ff = ...f ff ^ ...0 ff = ...f 00 = -100
// -ff ^ + 1 = ...f 01 ^ ...0 01 = ...f 00 = -100
// answer is neg, has length of longest with a possible carry
fn bitxor_neg_pos(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    let mut carry_xor = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        *ai = negate_carry(twos_a ^ bi, &mut carry_xor);
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => {
            for ai in a[b.len()..].iter_mut() {
                let twos_a = negate_carry(*ai, &mut carry_a);
                *ai = negate_carry(twos_a, &mut carry_xor);
            }
            debug_assert!(carry_a == 0);
        }
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().map(|&bi| {
                let twos_a = !0;
                negate_carry(twos_a ^ bi, &mut carry_xor)
            }));
        }
    }
    if carry_xor != 0 {
        a.push(1);
    }
}

// - 1 ^ -ff = ...f ff ^ ...f 01 = ...0 fe = +fe
// -ff & - 1 = ...f 01 ^ ...f ff = ...0 fe = +fe
// answer is pos, has length of longest
fn bitxor_neg_neg(a: &mut Vec<BigDigit>, b: &[BigDigit]) {
    let mut carry_a = 1;
    let mut carry_b = 1;
    for (ai, &bi) in a.iter_mut().zip(b.iter()) {
        let twos_a = negate_carry(*ai, &mut carry_a);
        let twos_b = negate_carry(bi, &mut carry_b);
        *ai = twos_a ^ twos_b;
    }
    debug_assert!(a.len() > b.len() || carry_a == 0);
    debug_assert!(b.len() > a.len() || carry_b == 0);
    match Ord::cmp(&a.len(), &b.len()) {
        Greater => {
            for ai in a[b.len()..].iter_mut() {
                let twos_a = negate_carry(*ai, &mut carry_a);
                let twos_b = !0;
                *ai = twos_a ^ twos_b;
            }
            debug_assert!(carry_a == 0);
        }
        Equal => {}
        Less => {
            let extra = &b[a.len()..];
            a.extend(extra.iter().map(|&bi| {
                let twos_a = !0;
                let twos_b = negate_carry(bi, &mut carry_b);
                twos_a ^ twos_b
            }));
            debug_assert!(carry_b == 0);
        }
    }
}

forward_all_binop_to_val_ref_commutative!(impl BitXor for BigInt, bitxor);

impl<'a> BitXor<&'a BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn bitxor(mut self, other: &BigInt) -> BigInt {
        self ^= other;
        self
    }
}

forward_val_assign!(impl BitXorAssign for BigInt, bitxor_assign);

impl<'a> BitXorAssign<&'a BigInt> for BigInt {
    fn bitxor_assign(&mut self, other: &BigInt) {
        match (self.sign, other.sign) {
            (_, NoSign) => {}
            (NoSign, _) => self.clone_from(other),
            (Plus, Plus) => {
                self.data ^= &other.data;
                if self.data.is_zero() {
                    self.sign = NoSign;
                }
            }
            (Plus, Minus) => {
                bitxor_pos_neg(self.digits_mut(), other.digits());
                self.sign = Minus;
                self.normalize();
            }
            (Minus, Plus) => {
                bitxor_neg_pos(self.digits_mut(), other.digits());
                self.normalize();
            }
            (Minus, Minus) => {
                bitxor_neg_neg(self.digits_mut(), other.digits());
                self.sign = Plus;
                self.normalize();
            }
        }
    }
}

impl FromStr for BigInt {
    type Err = ParseBigIntError;

    #[inline]
    fn from_str(s: &str) -> Result<BigInt, ParseBigIntError> {
        BigInt::from_str_radix(s, 10)
    }
}

impl Num for BigInt {
    type FromStrRadixErr = ParseBigIntError;

    /// Creates and initializes a BigInt.
    #[inline]
    fn from_str_radix(mut s: &str, radix: u32) -> Result<BigInt, ParseBigIntError> {
        let sign = if s.starts_with('-') {
            let tail = &s[1..];
            if !tail.starts_with('+') {
                s = tail
            }
            Minus
        } else {
            Plus
        };
        let bu = BigUint::from_str_radix(s, radix)?;
        Ok(BigInt::from_biguint(sign, bu))
    }
}

macro_rules! impl_shift {
    (@ref $Shx:ident :: $shx:ident, $ShxAssign:ident :: $shx_assign:ident, $rhs:ty) => {
        impl<'b> $Shx<&'b $rhs> for BigInt {
            type Output = BigInt;

            #[inline]
            fn $shx(self, rhs: &'b $rhs) -> BigInt {
                $Shx::$shx(self, *rhs)
            }
        }
        impl<'a, 'b> $Shx<&'b $rhs> for &'a BigInt {
            type Output = BigInt;

            #[inline]
            fn $shx(self, rhs: &'b $rhs) -> BigInt {
                $Shx::$shx(self, *rhs)
            }
        }
        impl<'b> $ShxAssign<&'b $rhs> for BigInt {
            #[inline]
            fn $shx_assign(&mut self, rhs: &'b $rhs) {
                $ShxAssign::$shx_assign(self, *rhs);
            }
        }
    };
    ($($rhs:ty),+) => {$(
        impl Shl<$rhs> for BigInt {
            type Output = BigInt;

            #[inline]
            fn shl(self, rhs: $rhs) -> BigInt {
                BigInt::from_biguint(self.sign, self.data << rhs)
            }
        }
        impl<'a> Shl<$rhs> for &'a BigInt {
            type Output = BigInt;

            #[inline]
            fn shl(self, rhs: $rhs) -> BigInt {
                BigInt::from_biguint(self.sign, &self.data << rhs)
            }
        }
        impl ShlAssign<$rhs> for BigInt {
            #[inline]
            fn shl_assign(&mut self, rhs: $rhs) {
                self.data <<= rhs
            }
        }
        impl_shift! { @ref Shl::shl, ShlAssign::shl_assign, $rhs }

        impl Shr<$rhs> for BigInt {
            type Output = BigInt;

            #[inline]
            fn shr(self, rhs: $rhs) -> BigInt {
                let round_down = shr_round_down(&self, rhs);
                let data = self.data >> rhs;
                let data = if round_down { data + 1u8 } else { data };
                BigInt::from_biguint(self.sign, data)
            }
        }
        impl<'a> Shr<$rhs> for &'a BigInt {
            type Output = BigInt;

            #[inline]
            fn shr(self, rhs: $rhs) -> BigInt {
                let round_down = shr_round_down(self, rhs);
                let data = &self.data >> rhs;
                let data = if round_down { data + 1u8 } else { data };
                BigInt::from_biguint(self.sign, data)
            }
        }
        impl ShrAssign<$rhs> for BigInt {
            #[inline]
            fn shr_assign(&mut self, rhs: $rhs) {
                let round_down = shr_round_down(self, rhs);
                self.data >>= rhs;
                if round_down {
                    self.data += 1u8;
                } else if self.data.is_zero() {
                    self.sign = NoSign;
                }
            }
        }
        impl_shift! { @ref Shr::shr, ShrAssign::shr_assign, $rhs }
    )*};
}

impl_shift! { u8, u16, u32, u64, u128, usize }
impl_shift! { i8, i16, i32, i64, i128, isize }

// Negative values need a rounding adjustment if there are any ones in the
// bits that are getting shifted out.
fn shr_round_down<T: PrimInt>(i: &BigInt, shift: T) -> bool {
    if i.is_negative() {
        let zeros = i.trailing_zeros().expect("negative values are non-zero");
        shift > T::zero() && shift.to_u64().map(|shift| zeros < shift).unwrap_or(true)
    } else {
        false
    }
}

impl Zero for BigInt {
    #[inline]
    fn zero() -> BigInt {
        BigInt {
            sign: NoSign,
            data: BigUint::zero(),
        }
    }

    #[inline]
    fn set_zero(&mut self) {
        self.data.set_zero();
        self.sign = NoSign;
    }

    #[inline]
    fn is_zero(&self) -> bool {
        self.sign == NoSign
    }
}

impl One for BigInt {
    #[inline]
    fn one() -> BigInt {
        BigInt {
            sign: Plus,
            data: BigUint::one(),
        }
    }

    #[inline]
    fn set_one(&mut self) {
        self.data.set_one();
        self.sign = Plus;
    }

    #[inline]
    fn is_one(&self) -> bool {
        self.sign == Plus && self.data.is_one()
    }
}

impl Signed for BigInt {
    #[inline]
    fn abs(&self) -> BigInt {
        match self.sign {
            Plus | NoSign => self.clone(),
            Minus => BigInt::from(self.data.clone()),
        }
    }

    #[inline]
    fn abs_sub(&self, other: &BigInt) -> BigInt {
        if *self <= *other {
            Zero::zero()
        } else {
            self - other
        }
    }

    #[inline]
    fn signum(&self) -> BigInt {
        match self.sign {
            Plus => BigInt::one(),
            Minus => -BigInt::one(),
            NoSign => BigInt::zero(),
        }
    }

    #[inline]
    fn is_positive(&self) -> bool {
        self.sign == Plus
    }

    #[inline]
    fn is_negative(&self) -> bool {
        self.sign == Minus
    }
}

/// Help function for pow
///
/// Computes the effect of the exponent on the sign.
#[inline]
fn powsign<T: Integer>(sign: Sign, other: &T) -> Sign {
    if other.is_zero() {
        Plus
    } else if sign != Minus || other.is_odd() {
        sign
    } else {
        -sign
    }
}

macro_rules! pow_impl {
    ($T:ty) => {
        impl Pow<$T> for BigInt {
            type Output = BigInt;

            #[inline]
            fn pow(self, rhs: $T) -> BigInt {
                BigInt::from_biguint(powsign(self.sign, &rhs), self.data.pow(rhs))
            }
        }

        impl<'b> Pow<&'b $T> for BigInt {
            type Output = BigInt;

            #[inline]
            fn pow(self, rhs: &$T) -> BigInt {
                BigInt::from_biguint(powsign(self.sign, rhs), self.data.pow(rhs))
            }
        }

        impl<'a> Pow<$T> for &'a BigInt {
            type Output = BigInt;

            #[inline]
            fn pow(self, rhs: $T) -> BigInt {
                BigInt::from_biguint(powsign(self.sign, &rhs), Pow::pow(&self.data, rhs))
            }
        }

        impl<'a, 'b> Pow<&'b $T> for &'a BigInt {
            type Output = BigInt;

            #[inline]
            fn pow(self, rhs: &$T) -> BigInt {
                BigInt::from_biguint(powsign(self.sign, rhs), Pow::pow(&self.data, rhs))
            }
        }
    };
}

pow_impl!(u8);
pow_impl!(u16);
pow_impl!(u32);
pow_impl!(u64);
pow_impl!(usize);
pow_impl!(u128);
pow_impl!(BigUint);

trait UnsignedAbs {
    type Unsigned;

    /// A convenience method for getting the absolute value of a signed primitive as unsigned
    /// See also `unsigned_abs`: https://github.com/rust-lang/rust/issues/74913
    fn uabs(self) -> Self::Unsigned;

    fn checked_uabs(self) -> CheckedUnsignedAbs<Self::Unsigned>;
}

enum CheckedUnsignedAbs<T> {
    Positive(T),
    Negative(T),
}
use self::CheckedUnsignedAbs::{Negative, Positive};

macro_rules! impl_unsigned_abs {
    ($Signed:ty, $Unsigned:ty) => {
        impl UnsignedAbs for $Signed {
            type Unsigned = $Unsigned;

            #[inline]
            fn uabs(self) -> $Unsigned {
                self.wrapping_abs() as $Unsigned
            }

            #[inline]
            fn checked_uabs(self) -> CheckedUnsignedAbs<Self::Unsigned> {
                if self >= 0 {
                    Positive(self as $Unsigned)
                } else {
                    Negative(self.wrapping_neg() as $Unsigned)
                }
            }
        }
    };
}
impl_unsigned_abs!(i8, u8);
impl_unsigned_abs!(i16, u16);
impl_unsigned_abs!(i32, u32);
impl_unsigned_abs!(i64, u64);
impl_unsigned_abs!(i128, u128);
impl_unsigned_abs!(isize, usize);

// We want to forward to BigUint::add, but it's not clear how that will go until
// we compare both sign and magnitude.  So we duplicate this body for every
// val/ref combination, deferring that decision to BigUint's own forwarding.
macro_rules! bigint_add {
    ($a:expr, $a_owned:expr, $a_data:expr, $b:expr, $b_owned:expr, $b_data:expr) => {
        match ($a.sign, $b.sign) {
            (_, NoSign) => $a_owned,
            (NoSign, _) => $b_owned,
            // same sign => keep the sign with the sum of magnitudes
            (Plus, Plus) | (Minus, Minus) => BigInt::from_biguint($a.sign, $a_data + $b_data),
            // opposite signs => keep the sign of the larger with the difference of magnitudes
            (Plus, Minus) | (Minus, Plus) => match $a.data.cmp(&$b.data) {
                Less => BigInt::from_biguint($b.sign, $b_data - $a_data),
                Greater => BigInt::from_biguint($a.sign, $a_data - $b_data),
                Equal => Zero::zero(),
            },
        }
    };
}

impl<'a, 'b> Add<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: &BigInt) -> BigInt {
        bigint_add!(
            self,
            self.clone(),
            &self.data,
            other,
            other.clone(),
            &other.data
        )
    }
}

impl<'a> Add<BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: BigInt) -> BigInt {
        bigint_add!(self, self.clone(), &self.data, other, other, other.data)
    }
}

impl<'a> Add<&'a BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: &BigInt) -> BigInt {
        bigint_add!(self, self, self.data, other, other.clone(), &other.data)
    }
}

impl Add<BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: BigInt) -> BigInt {
        bigint_add!(self, self, self.data, other, other, other.data)
    }
}

impl<'a> AddAssign<&'a BigInt> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: &BigInt) {
        let n = mem::replace(self, BigInt::zero());
        *self = n + other;
    }
}
forward_val_assign!(impl AddAssign for BigInt, add_assign);

promote_all_scalars!(impl Add for BigInt, add);
promote_all_scalars_assign!(impl AddAssign for BigInt, add_assign);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u32> for BigInt, add);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u64> for BigInt, add);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<u128> for BigInt, add);

impl Add<u32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: u32) -> BigInt {
        match self.sign {
            NoSign => From::from(other),
            Plus => BigInt::from(self.data + other),
            Minus => match self.data.cmp(&From::from(other)) {
                Equal => Zero::zero(),
                Less => BigInt::from(other - self.data),
                Greater => -BigInt::from(self.data - other),
            },
        }
    }
}

impl AddAssign<u32> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: u32) {
        let n = mem::replace(self, BigInt::zero());
        *self = n + other;
    }
}

impl Add<u64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: u64) -> BigInt {
        match self.sign {
            NoSign => From::from(other),
            Plus => BigInt::from(self.data + other),
            Minus => match self.data.cmp(&From::from(other)) {
                Equal => Zero::zero(),
                Less => BigInt::from(other - self.data),
                Greater => -BigInt::from(self.data - other),
            },
        }
    }
}

impl AddAssign<u64> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: u64) {
        let n = mem::replace(self, BigInt::zero());
        *self = n + other;
    }
}

impl Add<u128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: u128) -> BigInt {
        match self.sign {
            NoSign => BigInt::from(other),
            Plus => BigInt::from(self.data + other),
            Minus => match self.data.cmp(&From::from(other)) {
                Equal => BigInt::zero(),
                Less => BigInt::from(other - self.data),
                Greater => -BigInt::from(self.data - other),
            },
        }
    }
}
impl AddAssign<u128> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: u128) {
        let n = mem::replace(self, BigInt::zero());
        *self = n + other;
    }
}

forward_all_scalar_binop_to_val_val_commutative!(impl Add<i32> for BigInt, add);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<i64> for BigInt, add);
forward_all_scalar_binop_to_val_val_commutative!(impl Add<i128> for BigInt, add);

impl Add<i32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: i32) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self + u,
            Negative(u) => self - u,
        }
    }
}
impl AddAssign<i32> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: i32) {
        match other.checked_uabs() {
            Positive(u) => *self += u,
            Negative(u) => *self -= u,
        }
    }
}

impl Add<i64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: i64) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self + u,
            Negative(u) => self - u,
        }
    }
}
impl AddAssign<i64> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: i64) {
        match other.checked_uabs() {
            Positive(u) => *self += u,
            Negative(u) => *self -= u,
        }
    }
}

impl Add<i128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn add(self, other: i128) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self + u,
            Negative(u) => self - u,
        }
    }
}
impl AddAssign<i128> for BigInt {
    #[inline]
    fn add_assign(&mut self, other: i128) {
        match other.checked_uabs() {
            Positive(u) => *self += u,
            Negative(u) => *self -= u,
        }
    }
}

// We want to forward to BigUint::sub, but it's not clear how that will go until
// we compare both sign and magnitude.  So we duplicate this body for every
// val/ref combination, deferring that decision to BigUint's own forwarding.
macro_rules! bigint_sub {
    ($a:expr, $a_owned:expr, $a_data:expr, $b:expr, $b_owned:expr, $b_data:expr) => {
        match ($a.sign, $b.sign) {
            (_, NoSign) => $a_owned,
            (NoSign, _) => -$b_owned,
            // opposite signs => keep the sign of the left with the sum of magnitudes
            (Plus, Minus) | (Minus, Plus) => BigInt::from_biguint($a.sign, $a_data + $b_data),
            // same sign => keep or toggle the sign of the left with the difference of magnitudes
            (Plus, Plus) | (Minus, Minus) => match $a.data.cmp(&$b.data) {
                Less => BigInt::from_biguint(-$a.sign, $b_data - $a_data),
                Greater => BigInt::from_biguint($a.sign, $a_data - $b_data),
                Equal => Zero::zero(),
            },
        }
    };
}

impl<'a, 'b> Sub<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: &BigInt) -> BigInt {
        bigint_sub!(
            self,
            self.clone(),
            &self.data,
            other,
            other.clone(),
            &other.data
        )
    }
}

impl<'a> Sub<BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        bigint_sub!(self, self.clone(), &self.data, other, other, other.data)
    }
}

impl<'a> Sub<&'a BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: &BigInt) -> BigInt {
        bigint_sub!(self, self, self.data, other, other.clone(), &other.data)
    }
}

impl Sub<BigInt> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        bigint_sub!(self, self, self.data, other, other, other.data)
    }
}

impl<'a> SubAssign<&'a BigInt> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: &BigInt) {
        let n = mem::replace(self, BigInt::zero());
        *self = n - other;
    }
}
forward_val_assign!(impl SubAssign for BigInt, sub_assign);

promote_all_scalars!(impl Sub for BigInt, sub);
promote_all_scalars_assign!(impl SubAssign for BigInt, sub_assign);
forward_all_scalar_binop_to_val_val!(impl Sub<u32> for BigInt, sub);
forward_all_scalar_binop_to_val_val!(impl Sub<u64> for BigInt, sub);
forward_all_scalar_binop_to_val_val!(impl Sub<u128> for BigInt, sub);

impl Sub<u32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: u32) -> BigInt {
        match self.sign {
            NoSign => -BigInt::from(other),
            Minus => -BigInt::from(self.data + other),
            Plus => match self.data.cmp(&From::from(other)) {
                Equal => Zero::zero(),
                Greater => BigInt::from(self.data - other),
                Less => -BigInt::from(other - self.data),
            },
        }
    }
}
impl SubAssign<u32> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: u32) {
        let n = mem::replace(self, BigInt::zero());
        *self = n - other;
    }
}

impl Sub<BigInt> for u32 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        -(other - self)
    }
}

impl Sub<BigInt> for u64 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        -(other - self)
    }
}

impl Sub<BigInt> for u128 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        -(other - self)
    }
}

impl Sub<u64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: u64) -> BigInt {
        match self.sign {
            NoSign => -BigInt::from(other),
            Minus => -BigInt::from(self.data + other),
            Plus => match self.data.cmp(&From::from(other)) {
                Equal => Zero::zero(),
                Greater => BigInt::from(self.data - other),
                Less => -BigInt::from(other - self.data),
            },
        }
    }
}

impl SubAssign<u64> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: u64) {
        let n = mem::replace(self, BigInt::zero());
        *self = n - other;
    }
}

impl Sub<u128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: u128) -> BigInt {
        match self.sign {
            NoSign => -BigInt::from(other),
            Minus => -BigInt::from(self.data + other),
            Plus => match self.data.cmp(&From::from(other)) {
                Equal => Zero::zero(),
                Greater => BigInt::from(self.data - other),
                Less => -BigInt::from(other - self.data),
            },
        }
    }
}

impl SubAssign<u128> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: u128) {
        let n = mem::replace(self, BigInt::zero());
        *self = n - other;
    }
}

forward_all_scalar_binop_to_val_val!(impl Sub<i32> for BigInt, sub);
forward_all_scalar_binop_to_val_val!(impl Sub<i64> for BigInt, sub);
forward_all_scalar_binop_to_val_val!(impl Sub<i128> for BigInt, sub);

impl Sub<i32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: i32) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self - u,
            Negative(u) => self + u,
        }
    }
}
impl SubAssign<i32> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: i32) {
        match other.checked_uabs() {
            Positive(u) => *self -= u,
            Negative(u) => *self += u,
        }
    }
}

impl Sub<BigInt> for i32 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u - other,
            Negative(u) => -other - u,
        }
    }
}

impl Sub<i64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: i64) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self - u,
            Negative(u) => self + u,
        }
    }
}
impl SubAssign<i64> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: i64) {
        match other.checked_uabs() {
            Positive(u) => *self -= u,
            Negative(u) => *self += u,
        }
    }
}

impl Sub<BigInt> for i64 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u - other,
            Negative(u) => -other - u,
        }
    }
}

impl Sub<i128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: i128) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self - u,
            Negative(u) => self + u,
        }
    }
}

impl SubAssign<i128> for BigInt {
    #[inline]
    fn sub_assign(&mut self, other: i128) {
        match other.checked_uabs() {
            Positive(u) => *self -= u,
            Negative(u) => *self += u,
        }
    }
}

impl Sub<BigInt> for i128 {
    type Output = BigInt;

    #[inline]
    fn sub(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u - other,
            Negative(u) => -other - u,
        }
    }
}

forward_all_binop_to_ref_ref!(impl Mul for BigInt, mul);

impl<'a, 'b> Mul<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: &BigInt) -> BigInt {
        BigInt::from_biguint(self.sign * other.sign, &self.data * &other.data)
    }
}

impl<'a> MulAssign<&'a BigInt> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: &BigInt) {
        *self = &*self * other;
    }
}
forward_val_assign!(impl MulAssign for BigInt, mul_assign);

promote_all_scalars!(impl Mul for BigInt, mul);
promote_all_scalars_assign!(impl MulAssign for BigInt, mul_assign);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u32> for BigInt, mul);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u64> for BigInt, mul);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<u128> for BigInt, mul);

impl Mul<u32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: u32) -> BigInt {
        BigInt::from_biguint(self.sign, self.data * other)
    }
}

impl MulAssign<u32> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: u32) {
        self.data *= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Mul<u64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: u64) -> BigInt {
        BigInt::from_biguint(self.sign, self.data * other)
    }
}

impl MulAssign<u64> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: u64) {
        self.data *= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Mul<u128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: u128) -> BigInt {
        BigInt::from_biguint(self.sign, self.data * other)
    }
}

impl MulAssign<u128> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: u128) {
        self.data *= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

forward_all_scalar_binop_to_val_val_commutative!(impl Mul<i32> for BigInt, mul);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<i64> for BigInt, mul);
forward_all_scalar_binop_to_val_val_commutative!(impl Mul<i128> for BigInt, mul);

impl Mul<i32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: i32) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self * u,
            Negative(u) => -self * u,
        }
    }
}

impl MulAssign<i32> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: i32) {
        match other.checked_uabs() {
            Positive(u) => *self *= u,
            Negative(u) => {
                self.sign = -self.sign;
                self.data *= u;
            }
        }
    }
}

impl Mul<i64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: i64) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self * u,
            Negative(u) => -self * u,
        }
    }
}

impl MulAssign<i64> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: i64) {
        match other.checked_uabs() {
            Positive(u) => *self *= u,
            Negative(u) => {
                self.sign = -self.sign;
                self.data *= u;
            }
        }
    }
}

impl Mul<i128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn mul(self, other: i128) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self * u,
            Negative(u) => -self * u,
        }
    }
}

impl MulAssign<i128> for BigInt {
    #[inline]
    fn mul_assign(&mut self, other: i128) {
        match other.checked_uabs() {
            Positive(u) => *self *= u,
            Negative(u) => {
                self.sign = -self.sign;
                self.data *= u;
            }
        }
    }
}

forward_all_binop_to_ref_ref!(impl Div for BigInt, div);

impl<'a, 'b> Div<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: &BigInt) -> BigInt {
        let (q, _) = self.div_rem(other);
        q
    }
}

impl<'a> DivAssign<&'a BigInt> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: &BigInt) {
        *self = &*self / other;
    }
}
forward_val_assign!(impl DivAssign for BigInt, div_assign);

promote_all_scalars!(impl Div for BigInt, div);
promote_all_scalars_assign!(impl DivAssign for BigInt, div_assign);
forward_all_scalar_binop_to_val_val!(impl Div<u32> for BigInt, div);
forward_all_scalar_binop_to_val_val!(impl Div<u64> for BigInt, div);
forward_all_scalar_binop_to_val_val!(impl Div<u128> for BigInt, div);

impl Div<u32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: u32) -> BigInt {
        BigInt::from_biguint(self.sign, self.data / other)
    }
}

impl DivAssign<u32> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: u32) {
        self.data /= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Div<BigInt> for u32 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        BigInt::from_biguint(other.sign, self / other.data)
    }
}

impl Div<u64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: u64) -> BigInt {
        BigInt::from_biguint(self.sign, self.data / other)
    }
}

impl DivAssign<u64> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: u64) {
        self.data /= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Div<BigInt> for u64 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        BigInt::from_biguint(other.sign, self / other.data)
    }
}

impl Div<u128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: u128) -> BigInt {
        BigInt::from_biguint(self.sign, self.data / other)
    }
}

impl DivAssign<u128> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: u128) {
        self.data /= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Div<BigInt> for u128 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        BigInt::from_biguint(other.sign, self / other.data)
    }
}

forward_all_scalar_binop_to_val_val!(impl Div<i32> for BigInt, div);
forward_all_scalar_binop_to_val_val!(impl Div<i64> for BigInt, div);
forward_all_scalar_binop_to_val_val!(impl Div<i128> for BigInt, div);

impl Div<i32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: i32) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self / u,
            Negative(u) => -self / u,
        }
    }
}

impl DivAssign<i32> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: i32) {
        match other.checked_uabs() {
            Positive(u) => *self /= u,
            Negative(u) => {
                self.sign = -self.sign;
                *self /= u;
            }
        }
    }
}

impl Div<BigInt> for i32 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u / other,
            Negative(u) => u / -other,
        }
    }
}

impl Div<i64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: i64) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self / u,
            Negative(u) => -self / u,
        }
    }
}

impl DivAssign<i64> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: i64) {
        match other.checked_uabs() {
            Positive(u) => *self /= u,
            Negative(u) => {
                self.sign = -self.sign;
                *self /= u;
            }
        }
    }
}

impl Div<BigInt> for i64 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u / other,
            Negative(u) => u / -other,
        }
    }
}

impl Div<i128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn div(self, other: i128) -> BigInt {
        match other.checked_uabs() {
            Positive(u) => self / u,
            Negative(u) => -self / u,
        }
    }
}

impl DivAssign<i128> for BigInt {
    #[inline]
    fn div_assign(&mut self, other: i128) {
        match other.checked_uabs() {
            Positive(u) => *self /= u,
            Negative(u) => {
                self.sign = -self.sign;
                *self /= u;
            }
        }
    }
}

impl Div<BigInt> for i128 {
    type Output = BigInt;

    #[inline]
    fn div(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u / other,
            Negative(u) => u / -other,
        }
    }
}

forward_all_binop_to_ref_ref!(impl Rem for BigInt, rem);

impl<'a, 'b> Rem<&'b BigInt> for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: &BigInt) -> BigInt {
        if let Some(other) = other.to_u32() {
            self % other
        } else if let Some(other) = other.to_i32() {
            self % other
        } else {
            let (_, r) = self.div_rem(other);
            r
        }
    }
}

impl<'a> RemAssign<&'a BigInt> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: &BigInt) {
        *self = &*self % other;
    }
}
forward_val_assign!(impl RemAssign for BigInt, rem_assign);

promote_all_scalars!(impl Rem for BigInt, rem);
promote_all_scalars_assign!(impl RemAssign for BigInt, rem_assign);
forward_all_scalar_binop_to_val_val!(impl Rem<u32> for BigInt, rem);
forward_all_scalar_binop_to_val_val!(impl Rem<u64> for BigInt, rem);
forward_all_scalar_binop_to_val_val!(impl Rem<u128> for BigInt, rem);

impl Rem<u32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: u32) -> BigInt {
        BigInt::from_biguint(self.sign, self.data % other)
    }
}

impl RemAssign<u32> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: u32) {
        self.data %= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Rem<BigInt> for u32 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        BigInt::from(self % other.data)
    }
}

impl Rem<u64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: u64) -> BigInt {
        BigInt::from_biguint(self.sign, self.data % other)
    }
}

impl RemAssign<u64> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: u64) {
        self.data %= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Rem<BigInt> for u64 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        BigInt::from(self % other.data)
    }
}

impl Rem<u128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: u128) -> BigInt {
        BigInt::from_biguint(self.sign, self.data % other)
    }
}

impl RemAssign<u128> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: u128) {
        self.data %= other;
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
}

impl Rem<BigInt> for u128 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        BigInt::from(self % other.data)
    }
}

forward_all_scalar_binop_to_val_val!(impl Rem<i32> for BigInt, rem);
forward_all_scalar_binop_to_val_val!(impl Rem<i64> for BigInt, rem);
forward_all_scalar_binop_to_val_val!(impl Rem<i128> for BigInt, rem);

impl Rem<i32> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: i32) -> BigInt {
        self % other.uabs()
    }
}

impl RemAssign<i32> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: i32) {
        *self %= other.uabs();
    }
}

impl Rem<BigInt> for i32 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u % other,
            Negative(u) => -(u % other),
        }
    }
}

impl Rem<i64> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: i64) -> BigInt {
        self % other.uabs()
    }
}

impl RemAssign<i64> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: i64) {
        *self %= other.uabs();
    }
}

impl Rem<BigInt> for i64 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u % other,
            Negative(u) => -(u % other),
        }
    }
}

impl Rem<i128> for BigInt {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: i128) -> BigInt {
        self % other.uabs()
    }
}

impl RemAssign<i128> for BigInt {
    #[inline]
    fn rem_assign(&mut self, other: i128) {
        *self %= other.uabs();
    }
}

impl Rem<BigInt> for i128 {
    type Output = BigInt;

    #[inline]
    fn rem(self, other: BigInt) -> BigInt {
        match self.checked_uabs() {
            Positive(u) => u % other,
            Negative(u) => -(u % other),
        }
    }
}

impl Neg for BigInt {
    type Output = BigInt;

    #[inline]
    fn neg(mut self) -> BigInt {
        self.sign = -self.sign;
        self
    }
}

impl<'a> Neg for &'a BigInt {
    type Output = BigInt;

    #[inline]
    fn neg(self) -> BigInt {
        -self.clone()
    }
}

impl CheckedAdd for BigInt {
    #[inline]
    fn checked_add(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.add(v))
    }
}

impl CheckedSub for BigInt {
    #[inline]
    fn checked_sub(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.sub(v))
    }
}

impl CheckedMul for BigInt {
    #[inline]
    fn checked_mul(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.mul(v))
    }
}

impl CheckedDiv for BigInt {
    #[inline]
    fn checked_div(&self, v: &BigInt) -> Option<BigInt> {
        if v.is_zero() {
            return None;
        }
        Some(self.div(v))
    }
}

impl Integer for BigInt {
    #[inline]
    fn div_rem(&self, other: &BigInt) -> (BigInt, BigInt) {
        // r.sign == self.sign
        let (d_ui, r_ui) = self.data.div_rem(&other.data);
        let d = BigInt::from_biguint(self.sign, d_ui);
        let r = BigInt::from_biguint(self.sign, r_ui);
        if other.is_negative() {
            (-d, r)
        } else {
            (d, r)
        }
    }

    #[inline]
    fn div_floor(&self, other: &BigInt) -> BigInt {
        let (d_ui, m) = self.data.div_mod_floor(&other.data);
        let d = BigInt::from(d_ui);
        match (self.sign, other.sign) {
            (Plus, Plus) | (NoSign, Plus) | (Minus, Minus) => d,
            (Plus, Minus) | (NoSign, Minus) | (Minus, Plus) => {
                if m.is_zero() {
                    -d
                } else {
                    -d - 1u32
                }
            }
            (_, NoSign) => unreachable!(),
        }
    }

    #[inline]
    fn mod_floor(&self, other: &BigInt) -> BigInt {
        // m.sign == other.sign
        let m_ui = self.data.mod_floor(&other.data);
        let m = BigInt::from_biguint(other.sign, m_ui);
        match (self.sign, other.sign) {
            (Plus, Plus) | (NoSign, Plus) | (Minus, Minus) => m,
            (Plus, Minus) | (NoSign, Minus) | (Minus, Plus) => {
                if m.is_zero() {
                    m
                } else {
                    other - m
                }
            }
            (_, NoSign) => unreachable!(),
        }
    }

    fn div_mod_floor(&self, other: &BigInt) -> (BigInt, BigInt) {
        // m.sign == other.sign
        let (d_ui, m_ui) = self.data.div_mod_floor(&other.data);
        let d = BigInt::from(d_ui);
        let m = BigInt::from_biguint(other.sign, m_ui);
        match (self.sign, other.sign) {
            (Plus, Plus) | (NoSign, Plus) | (Minus, Minus) => (d, m),
            (Plus, Minus) | (NoSign, Minus) | (Minus, Plus) => {
                if m.is_zero() {
                    (-d, m)
                } else {
                    (-d - 1u32, other - m)
                }
            }
            (_, NoSign) => unreachable!(),
        }
    }

    #[inline]
    fn div_ceil(&self, other: &Self) -> Self {
        let (d_ui, m) = self.data.div_mod_floor(&other.data);
        let d = BigInt::from(d_ui);
        match (self.sign, other.sign) {
            (Plus, Minus) | (NoSign, Minus) | (Minus, Plus) => -d,
            (Plus, Plus) | (NoSign, Plus) | (Minus, Minus) => {
                if m.is_zero() {
                    d
                } else {
                    d + 1u32
                }
            }
            (_, NoSign) => unreachable!(),
        }
    }

    /// Calculates the Greatest Common Divisor (GCD) of the number and `other`.
    ///
    /// The result is always positive.
    #[inline]
    fn gcd(&self, other: &BigInt) -> BigInt {
        BigInt::from(self.data.gcd(&other.data))
    }

    /// Calculates the Lowest Common Multiple (LCM) of the number and `other`.
    #[inline]
    fn lcm(&self, other: &BigInt) -> BigInt {
        BigInt::from(self.data.lcm(&other.data))
    }

    /// Calculates the Greatest Common Divisor (GCD) and
    /// Lowest Common Multiple (LCM) together.
    #[inline]
    fn gcd_lcm(&self, other: &BigInt) -> (BigInt, BigInt) {
        let (gcd, lcm) = self.data.gcd_lcm(&other.data);
        (BigInt::from(gcd), BigInt::from(lcm))
    }

    /// Greatest common divisor, least common multiple, and B??zout coefficients.
    #[inline]
    fn extended_gcd_lcm(&self, other: &BigInt) -> (num_integer::ExtendedGcd<BigInt>, BigInt) {
        let egcd = self.extended_gcd(other);
        let lcm = if egcd.gcd.is_zero() {
            BigInt::zero()
        } else {
            BigInt::from(&self.data / &egcd.gcd.data * &other.data)
        };
        (egcd, lcm)
    }

    /// Deprecated, use `is_multiple_of` instead.
    #[inline]
    fn divides(&self, other: &BigInt) -> bool {
        self.is_multiple_of(other)
    }

    /// Returns `true` if the number is a multiple of `other`.
    #[inline]
    fn is_multiple_of(&self, other: &BigInt) -> bool {
        self.data.is_multiple_of(&other.data)
    }

    /// Returns `true` if the number is divisible by `2`.
    #[inline]
    fn is_even(&self) -> bool {
        self.data.is_even()
    }

    /// Returns `true` if the number is not divisible by `2`.
    #[inline]
    fn is_odd(&self) -> bool {
        self.data.is_odd()
    }

    /// Rounds up to nearest multiple of argument.
    #[inline]
    fn next_multiple_of(&self, other: &Self) -> Self {
        let m = self.mod_floor(other);
        if m.is_zero() {
            self.clone()
        } else {
            self + (other - m)
        }
    }
    /// Rounds down to nearest multiple of argument.
    #[inline]
    fn prev_multiple_of(&self, other: &Self) -> Self {
        self - self.mod_floor(other)
    }
}

impl Roots for BigInt {
    fn nth_root(&self, n: u32) -> Self {
        assert!(
            !(self.is_negative() && n.is_even()),
            "root of degree {} is imaginary",
            n
        );

        BigInt::from_biguint(self.sign, self.data.nth_root(n))
    }

    fn sqrt(&self) -> Self {
        assert!(!self.is_negative(), "square root is imaginary");

        BigInt::from_biguint(self.sign, self.data.sqrt())
    }

    fn cbrt(&self) -> Self {
        BigInt::from_biguint(self.sign, self.data.cbrt())
    }
}

impl ToPrimitive for BigInt {
    #[inline]
    fn to_i64(&self) -> Option<i64> {
        match self.sign {
            Plus => self.data.to_i64(),
            NoSign => Some(0),
            Minus => {
                let n = self.data.to_u64()?;
                let m: u64 = 1 << 63;
                match n.cmp(&m) {
                    Less => Some(-(n as i64)),
                    Equal => Some(i64::MIN),
                    Greater => None,
                }
            }
        }
    }

    #[inline]
    fn to_i128(&self) -> Option<i128> {
        match self.sign {
            Plus => self.data.to_i128(),
            NoSign => Some(0),
            Minus => {
                let n = self.data.to_u128()?;
                let m: u128 = 1 << 127;
                match n.cmp(&m) {
                    Less => Some(-(n as i128)),
                    Equal => Some(i128::MIN),
                    Greater => None,
                }
            }
        }
    }

    #[inline]
    fn to_u64(&self) -> Option<u64> {
        match self.sign {
            Plus => self.data.to_u64(),
            NoSign => Some(0),
            Minus => None,
        }
    }

    #[inline]
    fn to_u128(&self) -> Option<u128> {
        match self.sign {
            Plus => self.data.to_u128(),
            NoSign => Some(0),
            Minus => None,
        }
    }

    #[inline]
    fn to_f32(&self) -> Option<f32> {
        let n = self.data.to_f32()?;
        Some(if self.sign == Minus { -n } else { n })
    }

    #[inline]
    fn to_f64(&self) -> Option<f64> {
        let n = self.data.to_f64()?;
        Some(if self.sign == Minus { -n } else { n })
    }
}

macro_rules! impl_try_from_bigint {
    ($T:ty, $to_ty:path) => {
        #[cfg(has_try_from)]
        impl TryFrom<&BigInt> for $T {
            type Error = TryFromBigIntError<()>;

            #[inline]
            fn try_from(value: &BigInt) -> Result<$T, TryFromBigIntError<()>> {
                $to_ty(value).ok_or(TryFromBigIntError::new(()))
            }
        }

        #[cfg(has_try_from)]
        impl TryFrom<BigInt> for $T {
            type Error = TryFromBigIntError<BigInt>;

            #[inline]
            fn try_from(value: BigInt) -> Result<$T, TryFromBigIntError<BigInt>> {
                <$T>::try_from(&value).map_err(|_| TryFromBigIntError::new(value))
            }
        }
    };
}

impl_try_from_bigint!(u8, ToPrimitive::to_u8);
impl_try_from_bigint!(u16, ToPrimitive::to_u16);
impl_try_from_bigint!(u32, ToPrimitive::to_u32);
impl_try_from_bigint!(u64, ToPrimitive::to_u64);
impl_try_from_bigint!(usize, ToPrimitive::to_usize);
impl_try_from_bigint!(u128, ToPrimitive::to_u128);

impl_try_from_bigint!(i8, ToPrimitive::to_i8);
impl_try_from_bigint!(i16, ToPrimitive::to_i16);
impl_try_from_bigint!(i32, ToPrimitive::to_i32);
impl_try_from_bigint!(i64, ToPrimitive::to_i64);
impl_try_from_bigint!(isize, ToPrimitive::to_isize);
impl_try_from_bigint!(i128, ToPrimitive::to_i128);

impl FromPrimitive for BigInt {
    #[inline]
    fn from_i64(n: i64) -> Option<BigInt> {
        Some(BigInt::from(n))
    }

    #[inline]
    fn from_i128(n: i128) -> Option<BigInt> {
        Some(BigInt::from(n))
    }

    #[inline]
    fn from_u64(n: u64) -> Option<BigInt> {
        Some(BigInt::from(n))
    }

    #[inline]
    fn from_u128(n: u128) -> Option<BigInt> {
        Some(BigInt::from(n))
    }

    #[inline]
    fn from_f64(n: f64) -> Option<BigInt> {
        if n >= 0.0 {
            BigUint::from_f64(n).map(BigInt::from)
        } else {
            let x = BigUint::from_f64(-n)?;
            Some(-BigInt::from(x))
        }
    }
}

impl From<i64> for BigInt {
    #[inline]
    fn from(n: i64) -> Self {
        if n >= 0 {
            BigInt::from(n as u64)
        } else {
            let u = u64::MAX - (n as u64) + 1;
            BigInt {
                sign: Minus,
                data: BigUint::from(u),
            }
        }
    }
}

impl From<i128> for BigInt {
    #[inline]
    fn from(n: i128) -> Self {
        if n >= 0 {
            BigInt::from(n as u128)
        } else {
            let u = u128::MAX - (n as u128) + 1;
            BigInt {
                sign: Minus,
                data: BigUint::from(u),
            }
        }
    }
}

macro_rules! impl_bigint_from_int {
    ($T:ty) => {
        impl From<$T> for BigInt {
            #[inline]
            fn from(n: $T) -> Self {
                BigInt::from(n as i64)
            }
        }
    };
}

impl_bigint_from_int!(i8);
impl_bigint_from_int!(i16);
impl_bigint_from_int!(i32);
impl_bigint_from_int!(isize);

impl From<u64> for BigInt {
    #[inline]
    fn from(n: u64) -> Self {
        if n > 0 {
            BigInt {
                sign: Plus,
                data: BigUint::from(n),
            }
        } else {
            BigInt::zero()
        }
    }
}

impl From<u128> for BigInt {
    #[inline]
    fn from(n: u128) -> Self {
        if n > 0 {
            BigInt {
                sign: Plus,
                data: BigUint::from(n),
            }
        } else {
            BigInt::zero()
        }
    }
}

macro_rules! impl_bigint_from_uint {
    ($T:ty) => {
        impl From<$T> for BigInt {
            #[inline]
            fn from(n: $T) -> Self {
                BigInt::from(n as u64)
            }
        }
    };
}

impl_bigint_from_uint!(u8);
impl_bigint_from_uint!(u16);
impl_bigint_from_uint!(u32);
impl_bigint_from_uint!(usize);

impl From<BigUint> for BigInt {
    #[inline]
    fn from(n: BigUint) -> Self {
        if n.is_zero() {
            BigInt::zero()
        } else {
            BigInt {
                sign: Plus,
                data: n,
            }
        }
    }
}

impl IntDigits for BigInt {
    #[inline]
    fn digits(&self) -> &[BigDigit] {
        self.data.digits()
    }
    #[inline]
    fn digits_mut(&mut self) -> &mut Vec<BigDigit> {
        self.data.digits_mut()
    }
    #[inline]
    fn normalize(&mut self) {
        self.data.normalize();
        if self.data.is_zero() {
            self.sign = NoSign;
        }
    }
    #[inline]
    fn capacity(&self) -> usize {
        self.data.capacity()
    }
    #[inline]
    fn len(&self) -> usize {
        self.data.len()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for BigInt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Note: do not change the serialization format, or it may break
        // forward and backward compatibility of serialized data!
        (self.sign, &self.data).serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for BigInt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (sign, data) = serde::Deserialize::deserialize(deserializer)?;
        Ok(BigInt::from_biguint(sign, data))
    }
}

/// A generic trait for converting a value to a `BigInt`. This may return
/// `None` when converting from `f32` or `f64`, and will always succeed
/// when converting from any integer or unsigned primitive, or `BigUint`.
pub trait ToBigInt {
    /// Converts the value of `self` to a `BigInt`.
    fn to_bigint(&self) -> Option<BigInt>;
}

impl ToBigInt for BigInt {
    #[inline]
    fn to_bigint(&self) -> Option<BigInt> {
        Some(self.clone())
    }
}

impl ToBigInt for BigUint {
    #[inline]
    fn to_bigint(&self) -> Option<BigInt> {
        if self.is_zero() {
            Some(Zero::zero())
        } else {
            Some(BigInt {
                sign: Plus,
                data: self.clone(),
            })
        }
    }
}

impl biguint::ToBigUint for BigInt {
    #[inline]
    fn to_biguint(&self) -> Option<BigUint> {
        match self.sign() {
            Plus => Some(self.data.clone()),
            NoSign => Some(Zero::zero()),
            Minus => None,
        }
    }
}

#[cfg(has_try_from)]
impl TryFrom<&BigInt> for BigUint {
    type Error = TryFromBigIntError<()>;

    #[inline]
    fn try_from(value: &BigInt) -> Result<BigUint, TryFromBigIntError<()>> {
        value.to_biguint().ok_or(TryFromBigIntError::new(()))
    }
}

#[cfg(has_try_from)]
impl TryFrom<BigInt> for BigUint {
    type Error = TryFromBigIntError<BigInt>;

    #[inline]
    fn try_from(value: BigInt) -> Result<BigUint, TryFromBigIntError<BigInt>> {
        if value.sign() == Sign::Minus {
            Err(TryFromBigIntError::new(value))
        } else {
            Ok(value.data)
        }
    }
}

macro_rules! impl_to_bigint {
    ($T:ty, $from_ty:path) => {
        impl ToBigInt for $T {
            #[inline]
            fn to_bigint(&self) -> Option<BigInt> {
                $from_ty(*self)
            }
        }
    };
}

impl_to_bigint!(isize, FromPrimitive::from_isize);
impl_to_bigint!(i8, FromPrimitive::from_i8);
impl_to_bigint!(i16, FromPrimitive::from_i16);
impl_to_bigint!(i32, FromPrimitive::from_i32);
impl_to_bigint!(i64, FromPrimitive::from_i64);
impl_to_bigint!(i128, FromPrimitive::from_i128);

impl_to_bigint!(usize, FromPrimitive::from_usize);
impl_to_bigint!(u8, FromPrimitive::from_u8);
impl_to_bigint!(u16, FromPrimitive::from_u16);
impl_to_bigint!(u32, FromPrimitive::from_u32);
impl_to_bigint!(u64, FromPrimitive::from_u64);
impl_to_bigint!(u128, FromPrimitive::from_u128);

impl_to_bigint!(f32, FromPrimitive::from_f32);
impl_to_bigint!(f64, FromPrimitive::from_f64);

impl BigInt {
    /// Creates and initializes a BigInt.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn new(sign: Sign, digits: Vec<u32>) -> BigInt {
        BigInt::from_biguint(sign, BigUint::new(digits))
    }

    /// Creates and initializes a `BigInt`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn from_biguint(mut sign: Sign, mut data: BigUint) -> BigInt {
        if sign == NoSign {
            data.assign_from_slice(&[]);
        } else if data.is_zero() {
            sign = NoSign;
        }

        BigInt { sign, data }
    }

    /// Creates and initializes a `BigInt`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn from_slice(sign: Sign, slice: &[u32]) -> BigInt {
        BigInt::from_biguint(sign, BigUint::from_slice(slice))
    }

    /// Reinitializes a `BigInt`.
    ///
    /// The base 2<sup>32</sup> digits are ordered least significant digit first.
    #[inline]
    pub fn assign_from_slice(&mut self, sign: Sign, slice: &[u32]) {
        if sign == NoSign {
            self.set_zero();
        } else {
            self.data.assign_from_slice(slice);
            self.sign = if self.data.is_zero() { NoSign } else { sign };
        }
    }

    /// Creates and initializes a `BigInt`.
    ///
    /// The bytes are in big-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// assert_eq!(BigInt::from_bytes_be(Sign::Plus, b"A"),
    ///            BigInt::parse_bytes(b"65", 10).unwrap());
    /// assert_eq!(BigInt::from_bytes_be(Sign::Plus, b"AA"),
    ///            BigInt::parse_bytes(b"16705", 10).unwrap());
    /// assert_eq!(BigInt::from_bytes_be(Sign::Plus, b"AB"),
    ///            BigInt::parse_bytes(b"16706", 10).unwrap());
    /// assert_eq!(BigInt::from_bytes_be(Sign::Plus, b"Hello world!"),
    ///            BigInt::parse_bytes(b"22405534230753963835153736737", 10).unwrap());
    /// ```
    #[inline]
    pub fn from_bytes_be(sign: Sign, bytes: &[u8]) -> BigInt {
        BigInt::from_biguint(sign, BigUint::from_bytes_be(bytes))
    }

    /// Creates and initializes a `BigInt`.
    ///
    /// The bytes are in little-endian byte order.
    #[inline]
    pub fn from_bytes_le(sign: Sign, bytes: &[u8]) -> BigInt {
        BigInt::from_biguint(sign, BigUint::from_bytes_le(bytes))
    }

    /// Creates and initializes a `BigInt` from an array of bytes in
    /// two's complement binary representation.
    ///
    /// The digits are in big-endian base 2<sup>8</sup>.
    #[inline]
    pub fn from_signed_bytes_be(digits: &[u8]) -> BigInt {
        let sign = match digits.first() {
            Some(v) if *v > 0x7f => Sign::Minus,
            Some(_) => Sign::Plus,
            None => return BigInt::zero(),
        };

        if sign == Sign::Minus {
            // two's-complement the content to retrieve the magnitude
            let mut digits = Vec::from(digits);
            twos_complement_be(&mut digits);
            BigInt::from_biguint(sign, BigUint::from_bytes_be(&*digits))
        } else {
            BigInt::from_biguint(sign, BigUint::from_bytes_be(digits))
        }
    }

    /// Creates and initializes a `BigInt` from an array of bytes in two's complement.
    ///
    /// The digits are in little-endian base 2<sup>8</sup>.
    #[inline]
    pub fn from_signed_bytes_le(digits: &[u8]) -> BigInt {
        let sign = match digits.last() {
            Some(v) if *v > 0x7f => Sign::Minus,
            Some(_) => Sign::Plus,
            None => return BigInt::zero(),
        };

        if sign == Sign::Minus {
            // two's-complement the content to retrieve the magnitude
            let mut digits = Vec::from(digits);
            twos_complement_le(&mut digits);
            BigInt::from_biguint(sign, BigUint::from_bytes_le(&*digits))
        } else {
            BigInt::from_biguint(sign, BigUint::from_bytes_le(digits))
        }
    }

    /// Creates and initializes a `BigInt`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, ToBigInt};
    ///
    /// assert_eq!(BigInt::parse_bytes(b"1234", 10), ToBigInt::to_bigint(&1234));
    /// assert_eq!(BigInt::parse_bytes(b"ABCD", 16), ToBigInt::to_bigint(&0xABCD));
    /// assert_eq!(BigInt::parse_bytes(b"G", 16), None);
    /// ```
    #[inline]
    pub fn parse_bytes(buf: &[u8], radix: u32) -> Option<BigInt> {
        let s = str::from_utf8(buf).ok()?;
        BigInt::from_str_radix(s, radix).ok()
    }

    /// Creates and initializes a `BigInt`. Each u8 of the input slice is
    /// interpreted as one digit of the number
    /// and must therefore be less than `radix`.
    ///
    /// The bytes are in big-endian byte order.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// let inbase190 = vec![15, 33, 125, 12, 14];
    /// let a = BigInt::from_radix_be(Sign::Minus, &inbase190, 190).unwrap();
    /// assert_eq!(a.to_radix_be(190), (Sign:: Minus, inbase190));
    /// ```
    pub fn from_radix_be(sign: Sign, buf: &[u8], radix: u32) -> Option<BigInt> {
        let u = BigUint::from_radix_be(buf, radix)?;
        Some(BigInt::from_biguint(sign, u))
    }

    /// Creates and initializes a `BigInt`. Each u8 of the input slice is
    /// interpreted as one digit of the number
    /// and must therefore be less than `radix`.
    ///
    /// The bytes are in little-endian byte order.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// let inbase190 = vec![14, 12, 125, 33, 15];
    /// let a = BigInt::from_radix_be(Sign::Minus, &inbase190, 190).unwrap();
    /// assert_eq!(a.to_radix_be(190), (Sign::Minus, inbase190));
    /// ```
    pub fn from_radix_le(sign: Sign, buf: &[u8], radix: u32) -> Option<BigInt> {
        let u = BigUint::from_radix_le(buf, radix)?;
        Some(BigInt::from_biguint(sign, u))
    }

    /// Returns the sign and the byte representation of the `BigInt` in big-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{ToBigInt, Sign};
    ///
    /// let i = -1125.to_bigint().unwrap();
    /// assert_eq!(i.to_bytes_be(), (Sign::Minus, vec![4, 101]));
    /// ```
    #[inline]
    pub fn to_bytes_be(&self) -> (Sign, Vec<u8>) {
        (self.sign, self.data.to_bytes_be())
    }

    /// Returns the sign and the byte representation of the `BigInt` in little-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{ToBigInt, Sign};
    ///
    /// let i = -1125.to_bigint().unwrap();
    /// assert_eq!(i.to_bytes_le(), (Sign::Minus, vec![101, 4]));
    /// ```
    #[inline]
    pub fn to_bytes_le(&self) -> (Sign, Vec<u8>) {
        (self.sign, self.data.to_bytes_le())
    }

    /// Returns the sign and the `u32` digits representation of the `BigInt` ordered least
    /// significant digit first.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// assert_eq!(BigInt::from(-1125).to_u32_digits(), (Sign::Minus, vec![1125]));
    /// assert_eq!(BigInt::from(4294967295u32).to_u32_digits(), (Sign::Plus, vec![4294967295]));
    /// assert_eq!(BigInt::from(4294967296u64).to_u32_digits(), (Sign::Plus, vec![0, 1]));
    /// assert_eq!(BigInt::from(-112500000000i64).to_u32_digits(), (Sign::Minus, vec![830850304, 26]));
    /// assert_eq!(BigInt::from(112500000000i64).to_u32_digits(), (Sign::Plus, vec![830850304, 26]));
    /// ```
    #[inline]
    pub fn to_u32_digits(&self) -> (Sign, Vec<u32>) {
        (self.sign, self.data.to_u32_digits())
    }

    /// Returns the two's-complement byte representation of the `BigInt` in big-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::ToBigInt;
    ///
    /// let i = -1125.to_bigint().unwrap();
    /// assert_eq!(i.to_signed_bytes_be(), vec![251, 155]);
    /// ```
    #[inline]
    pub fn to_signed_bytes_be(&self) -> Vec<u8> {
        let mut bytes = self.data.to_bytes_be();
        let first_byte = bytes.first().cloned().unwrap_or(0);
        if first_byte > 0x7f
            && !(first_byte == 0x80
                && bytes.iter().skip(1).all(Zero::is_zero)
                && self.sign == Sign::Minus)
        {
            // msb used by magnitude, extend by 1 byte
            bytes.insert(0, 0);
        }
        if self.sign == Sign::Minus {
            twos_complement_be(&mut bytes);
        }
        bytes
    }

    /// Returns the two's-complement byte representation of the `BigInt` in little-endian byte order.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::ToBigInt;
    ///
    /// let i = -1125.to_bigint().unwrap();
    /// assert_eq!(i.to_signed_bytes_le(), vec![155, 251]);
    /// ```
    #[inline]
    pub fn to_signed_bytes_le(&self) -> Vec<u8> {
        let mut bytes = self.data.to_bytes_le();
        let last_byte = bytes.last().cloned().unwrap_or(0);
        if last_byte > 0x7f
            && !(last_byte == 0x80
                && bytes.iter().rev().skip(1).all(Zero::is_zero)
                && self.sign == Sign::Minus)
        {
            // msb used by magnitude, extend by 1 byte
            bytes.push(0);
        }
        if self.sign == Sign::Minus {
            twos_complement_le(&mut bytes);
        }
        bytes
    }

    /// Returns the integer formatted as a string in the given radix.
    /// `radix` must be in the range `2...36`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::BigInt;
    ///
    /// let i = BigInt::parse_bytes(b"ff", 16).unwrap();
    /// assert_eq!(i.to_str_radix(16), "ff");
    /// ```
    #[inline]
    pub fn to_str_radix(&self, radix: u32) -> String {
        let mut v = to_str_radix_reversed(&self.data, radix);

        if self.is_negative() {
            v.push(b'-');
        }

        v.reverse();
        unsafe { String::from_utf8_unchecked(v) }
    }

    /// Returns the integer in the requested base in big-endian digit order.
    /// The output is not given in a human readable alphabet but as a zero
    /// based u8 number.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// assert_eq!(BigInt::from(-0xFFFFi64).to_radix_be(159),
    ///            (Sign::Minus, vec![2, 94, 27]));
    /// // 0xFFFF = 65535 = 2*(159^2) + 94*159 + 27
    /// ```
    #[inline]
    pub fn to_radix_be(&self, radix: u32) -> (Sign, Vec<u8>) {
        (self.sign, self.data.to_radix_be(radix))
    }

    /// Returns the integer in the requested base in little-endian digit order.
    /// The output is not given in a human readable alphabet but as a zero
    /// based u8 number.
    /// `radix` must be in the range `2...256`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    ///
    /// assert_eq!(BigInt::from(-0xFFFFi64).to_radix_le(159),
    ///            (Sign::Minus, vec![27, 94, 2]));
    /// // 0xFFFF = 65535 = 27 + 94*159 + 2*(159^2)
    /// ```
    #[inline]
    pub fn to_radix_le(&self, radix: u32) -> (Sign, Vec<u8>) {
        (self.sign, self.data.to_radix_le(radix))
    }

    /// Returns the sign of the `BigInt` as a `Sign`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, Sign};
    /// use num_traits::Zero;
    ///
    /// assert_eq!(BigInt::from(1234).sign(), Sign::Plus);
    /// assert_eq!(BigInt::from(-4321).sign(), Sign::Minus);
    /// assert_eq!(BigInt::zero().sign(), Sign::NoSign);
    /// ```
    #[inline]
    pub fn sign(&self) -> Sign {
        self.sign
    }

    /// Returns the magnitude of the `BigInt` as a `BigUint`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, BigUint};
    /// use num_traits::Zero;
    ///
    /// assert_eq!(BigInt::from(1234).magnitude(), &BigUint::from(1234u32));
    /// assert_eq!(BigInt::from(-4321).magnitude(), &BigUint::from(4321u32));
    /// assert!(BigInt::zero().magnitude().is_zero());
    /// ```
    #[inline]
    pub fn magnitude(&self) -> &BigUint {
        &self.data
    }

    /// Convert this `BigInt` into its `Sign` and `BigUint` magnitude,
    /// the reverse of `BigInt::from_biguint`.
    ///
    /// # Examples
    ///
    /// ```
    /// use num_bigint::{BigInt, BigUint, Sign};
    /// use num_traits::Zero;
    ///
    /// assert_eq!(BigInt::from(1234).into_parts(), (Sign::Plus, BigUint::from(1234u32)));
    /// assert_eq!(BigInt::from(-4321).into_parts(), (Sign::Minus, BigUint::from(4321u32)));
    /// assert_eq!(BigInt::zero().into_parts(), (Sign::NoSign, BigUint::zero()));
    /// ```
    #[inline]
    pub fn into_parts(self) -> (Sign, BigUint) {
        (self.sign, self.data)
    }

    /// Determines the fewest bits necessary to express the `BigInt`,
    /// not including the sign.
    #[inline]
    pub fn bits(&self) -> u64 {
        self.data.bits()
    }

    /// Converts this `BigInt` into a `BigUint`, if it's not negative.
    #[inline]
    pub fn to_biguint(&self) -> Option<BigUint> {
        match self.sign {
            Plus => Some(self.data.clone()),
            NoSign => Some(Zero::zero()),
            Minus => None,
        }
    }

    #[inline]
    pub fn checked_add(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.add(v))
    }

    #[inline]
    pub fn checked_sub(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.sub(v))
    }

    #[inline]
    pub fn checked_mul(&self, v: &BigInt) -> Option<BigInt> {
        Some(self.mul(v))
    }

    #[inline]
    pub fn checked_div(&self, v: &BigInt) -> Option<BigInt> {
        if v.is_zero() {
            return None;
        }
        Some(self.div(v))
    }

    /// Returns `self ^ exponent`.
    pub fn pow(&self, exponent: u32) -> Self {
        Pow::pow(self, exponent)
    }

    /// Returns `(self ^ exponent) mod modulus`
    ///
    /// Note that this rounds like `mod_floor`, not like the `%` operator,
    /// which makes a difference when given a negative `self` or `modulus`.
    /// The result will be in the interval `[0, modulus)` for `modulus > 0`,
    /// or in the interval `(modulus, 0]` for `modulus < 0`
    ///
    /// Panics if the exponent is negative or the modulus is zero.
    pub fn modpow(&self, exponent: &Self, modulus: &Self) -> Self {
        assert!(
            !exponent.is_negative(),
            "negative exponentiation is not supported!"
        );
        assert!(
            !modulus.is_zero(),
            "attempt to calculate with zero modulus!"
        );

        let result = self.data.modpow(&exponent.data, &modulus.data);
        if result.is_zero() {
            return BigInt::zero();
        }

        // The sign of the result follows the modulus, like `mod_floor`.
        let (sign, mag) = match (
            self.is_negative() && exponent.is_odd(),
            modulus.is_negative(),
        ) {
            (false, false) => (Plus, result),
            (true, false) => (Plus, &modulus.data - result),
            (false, true) => (Minus, &modulus.data - result),
            (true, true) => (Minus, result),
        };
        BigInt::from_biguint(sign, mag)
    }

    /// Returns the truncated principal square root of `self` --
    /// see [Roots::sqrt](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#method.sqrt).
    pub fn sqrt(&self) -> Self {
        Roots::sqrt(self)
    }

    /// Returns the truncated principal cube root of `self` --
    /// see [Roots::cbrt](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#method.cbrt).
    pub fn cbrt(&self) -> Self {
        Roots::cbrt(self)
    }

    /// Returns the truncated principal `n`th root of `self` --
    /// See [Roots::nth_root](https://docs.rs/num-integer/0.1/num_integer/trait.Roots.html#tymethod.nth_root).
    pub fn nth_root(&self, n: u32) -> Self {
        Roots::nth_root(self, n)
    }

    /// Returns the number of least-significant bits that are zero,
    /// or `None` if the entire number is zero.
    pub fn trailing_zeros(&self) -> Option<u64> {
        self.data.trailing_zeros()
    }
}

impl_sum_iter_type!(BigInt);
impl_product_iter_type!(BigInt);

/// Perform in-place two's complement of the given binary representation,
/// in little-endian byte order.
#[inline]
fn twos_complement_le(digits: &mut [u8]) {
    twos_complement(digits)
}

/// Perform in-place two's complement of the given binary representation
/// in big-endian byte order.
#[inline]
fn twos_complement_be(digits: &mut [u8]) {
    twos_complement(digits.iter_mut().rev())
}

/// Perform in-place two's complement of the given digit iterator
/// starting from the least significant byte.
#[inline]
fn twos_complement<'a, I>(digits: I)
where
    I: IntoIterator<Item = &'a mut u8>,
{
    let mut carry = true;
    for d in digits {
        *d = d.not();
        if carry {
            *d = d.wrapping_add(1);
            carry = d.is_zero();
        }
    }
}

#[test]
fn test_from_biguint() {
    fn check(inp_s: Sign, inp_n: usize, ans_s: Sign, ans_n: usize) {
        let inp = BigInt::from_biguint(inp_s, FromPrimitive::from_usize(inp_n).unwrap());
        let ans = BigInt {
            sign: ans_s,
            data: FromPrimitive::from_usize(ans_n).unwrap(),
        };
        assert_eq!(inp, ans);
    }
    check(Plus, 1, Plus, 1);
    check(Plus, 0, NoSign, 0);
    check(Minus, 1, Minus, 1);
    check(NoSign, 1, NoSign, 0);
}

#[test]
fn test_from_slice() {
    fn check(inp_s: Sign, inp_n: u32, ans_s: Sign, ans_n: u32) {
        let inp = BigInt::from_slice(inp_s, &[inp_n]);
        let ans = BigInt {
            sign: ans_s,
            data: FromPrimitive::from_u32(ans_n).unwrap(),
        };
        assert_eq!(inp, ans);
    }
    check(Plus, 1, Plus, 1);
    check(Plus, 0, NoSign, 0);
    check(Minus, 1, Minus, 1);
    check(NoSign, 1, NoSign, 0);
}

#[test]
fn test_assign_from_slice() {
    fn check(inp_s: Sign, inp_n: u32, ans_s: Sign, ans_n: u32) {
        let mut inp = BigInt::from_slice(Minus, &[2627_u32, 0_u32, 9182_u32, 42_u32]);
        inp.assign_from_slice(inp_s, &[inp_n]);
        let ans = BigInt {
            sign: ans_s,
            data: FromPrimitive::from_u32(ans_n).unwrap(),
        };
        assert_eq!(inp, ans);
    }
    check(Plus, 1, Plus, 1);
    check(Plus, 0, NoSign, 0);
    check(Minus, 1, Minus, 1);
    check(NoSign, 1, NoSign, 0);
}
