use crate::core::amount::Amount;
use crate::core::data::{Balance, Close, Commodity, Document, Open, Options, Price, Transaction};
use crate::core::inventory::AccountName;
use crate::core::ledger::{
    AccountInfo, AccountSnapshot, AccountStatus, CurrencyInfo, DailyAccountSnapshot, DocumentType, Ledger, LedgerError,
};
use crate::core::utils::price_grip::DatedPriceGrip;
use crate::error::ZhangResult;
use chrono::NaiveDate;
use log::error;
use std::collections::HashMap;
use std::ops::{Neg, Sub};
use std::sync::{Arc, RwLock as StdRwLock};

pub(crate) struct ProcessContext {
    pub(crate) target_day: Option<NaiveDate>,
    pub(crate) prices: Arc<StdRwLock<DatedPriceGrip>>,
}

impl ProcessContext {
    pub fn default_account_snapshot(&self) -> AccountSnapshot {
        AccountSnapshot {
            inner: Default::default(),
            prices: self.prices.clone(),
        }
    }
}

pub(crate) trait DirectiveProcess {
    fn process(&mut self, ledger: &mut Ledger, context: &mut ProcessContext) -> ZhangResult<()>;
}

fn record_daily_snapshot(
    snapshot: &mut HashMap<AccountName, AccountSnapshot>, daily_snapshot: &mut DailyAccountSnapshot,
    target_day: &mut Option<NaiveDate>, date: NaiveDate,
) {
    if let Some(target_day_inner) = target_day {
        if date.ne(target_day_inner) {
            daily_snapshot.insert_snapshot(*target_day_inner, snapshot.clone());
            *target_day = Some(date);
        }
    } else {
        *target_day = Some(date);
    }
}

impl DirectiveProcess for Options {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        ledger
            .configs
            .insert(self.key.clone().to_plain_string(), self.value.clone().to_plain_string());
        Ok(())
    }
}

impl DirectiveProcess for Open {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        let account_info = ledger
            .accounts
            .entry(self.account.content.to_string())
            .or_insert_with(|| AccountInfo {
                currencies: Default::default(),
                status: AccountStatus::Open,
                meta: Default::default(),
            });
        account_info.status = AccountStatus::Open;
        for (meta_key, meta_value) in &self.meta {
            account_info
                .meta
                .insert(meta_key.clone(), meta_value.clone().to_plain_string());
        }
        for currency in &self.commodities {
            account_info.currencies.insert(currency.to_string());
        }
        Ok(())
    }
}

impl DirectiveProcess for Close {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        let account_info = ledger
            .accounts
            .entry(self.account.content.to_string())
            .or_insert_with(|| AccountInfo {
                currencies: Default::default(),
                status: AccountStatus::Open,
                meta: Default::default(),
            });
        account_info.status = AccountStatus::Close;
        for (meta_key, meta_value) in &self.meta {
            account_info
                .meta
                .insert(meta_key.clone(), meta_value.clone().to_plain_string());
        }
        Ok(())
    }
}

impl DirectiveProcess for Commodity {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        let _target_currency = ledger
            .currencies
            .entry(self.currency.to_string())
            .or_insert_with(|| CurrencyInfo {
                commodity: self.clone(),
                prices: HashMap::new(),
            });
        Ok(())
    }
}

impl DirectiveProcess for Transaction {
    fn process(&mut self, ledger: &mut Ledger, context: &mut ProcessContext) -> ZhangResult<()> {
        if !self.is_balance() {
            error!("trx is not balanced");
        }
        let date = self.date.naive_date();
        record_daily_snapshot(
            &mut ledger.snapshot,
            &mut ledger.daily_snapshot,
            &mut context.target_day,
            date,
        );

        for txn_posting in self.txn_postings() {
            let target_account_snapshot = ledger
                .snapshot
                .entry(txn_posting.account_name())
                .or_insert_with(|| context.default_account_snapshot());
            target_account_snapshot.add_amount(txn_posting.units());
        }
        Ok(())
    }
}

impl DirectiveProcess for Balance {
    fn process(&mut self, ledger: &mut Ledger, context: &mut ProcessContext) -> ZhangResult<()> {
        match self {
            Balance::BalanceCheck(balance_check) => {
                record_daily_snapshot(
                    &mut ledger.snapshot,
                    &mut ledger.daily_snapshot,
                    &mut context.target_day,
                    balance_check.date.naive_date(),
                );

                let default = ledger.default_account_snapshot();
                let target_account_snapshot = ledger.snapshot.get(balance_check.account.name()).unwrap_or(&default);

                let decimal = target_account_snapshot.get(&balance_check.amount.currency);
                balance_check.current_amount =
                    Some(Amount::new(decimal.clone(), balance_check.amount.currency.clone()));
                if decimal.ne(&balance_check.amount.number) {
                    balance_check.distance = Some(Amount::new(
                        (&balance_check.amount.number).sub(&decimal),
                        balance_check.amount.currency.clone(),
                    ));
                    ledger.errors.push(LedgerError::AccountBalanceCheckError {
                        account_name: balance_check.account.name().to_string(),
                        target: Amount::new(
                            balance_check.amount.number.clone(),
                            balance_check.amount.currency.clone(),
                        ),
                        current: Amount::new(decimal.clone(), balance_check.amount.currency.clone()),
                        distance: Amount::new(
                            (&balance_check.amount.number).sub(&decimal),
                            balance_check.amount.currency.clone(),
                        ),
                    });
                    error!(
                        "balance error: account {} balance to {} {} with distance {} {}(current is {} {})",
                        balance_check.account.name(),
                        &balance_check.amount.number,
                        &balance_check.amount.currency,
                        (&balance_check.amount.number).sub(&decimal),
                        &balance_check.amount.currency,
                        &decimal,
                        &balance_check.amount.currency
                    );
                }
            }
            Balance::BalancePad(balance_pad) => {
                record_daily_snapshot(
                    &mut ledger.snapshot,
                    &mut ledger.daily_snapshot,
                    &mut context.target_day,
                    balance_pad.date.naive_date(),
                );

                let target_account_snapshot = ledger
                    .snapshot
                    .entry(balance_pad.account.name().to_string())
                    .or_insert_with(|| context.default_account_snapshot());

                let source_amount = target_account_snapshot.get(&balance_pad.amount.currency);
                let source_target_amount = &balance_pad.amount.number;
                // source account
                let distance = source_target_amount.sub(source_amount);
                let neg_distance = (&distance).neg();
                target_account_snapshot.add_amount(Amount::new(distance, balance_pad.amount.currency.clone()));

                // add to pad
                let pad_account_snapshot = ledger
                    .snapshot
                    .entry(balance_pad.pad.name().to_string())
                    .or_insert_with(|| context.default_account_snapshot());
                pad_account_snapshot.add_amount(Amount::new(neg_distance, balance_pad.amount.currency.clone()));
            }
        }

        Ok(())
    }
}

impl DirectiveProcess for Document {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        ledger.documents.insert(
            self.filename.clone().to_plain_string(),
            DocumentType::AccountDocument {
                date: self.date.clone(),
                account: self.account.clone(),
                filename: self.filename.clone().to_plain_string(),
            },
        );
        Ok(())
    }
}

impl DirectiveProcess for Price {
    fn process(&mut self, ledger: &mut Ledger, _context: &mut ProcessContext) -> ZhangResult<()> {
        let mut result = ledger.prices.write().unwrap();
        result.insert(
            self.date.naive_datetime(),
            self.currency.clone(),
            self.amount.currency.clone(),
            self.amount.number.clone(),
        );
        Ok(())
    }
}
