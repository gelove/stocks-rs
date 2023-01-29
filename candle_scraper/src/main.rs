mod providers;
mod structs;

use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use chrono_tz::{Tz, US::Eastern};
use providers::Provider;
use structs::*;
use common::database;

impl database::ToQuery for Candle {
  fn insert(&self) -> (&str, Vec<(&str, &dyn rusqlite::ToSql)>) {
    let query = "
        INSERT OR REPLACE INTO candles (
          symbol,
          resolution,
          scraped_at,
          timestamp,
          open,
          high,
          low,
          close,
          volume
      ) VALUES (
          :symbol,
          :resolution,
          strftime('%s', 'now'),
          :timestamp,
          :open,
          :high,
          :low,
          :close,
          :volume
      )
    ";
    let params = rusqlite::named_params! {
      ":symbol": self.symbol,
      ":resolution": self.resolution,
      ":timestamp": self.timestamp,
      ":open": self.open,
      ":high": self.high,
      ":low": self.low,
      ":close": self.close,
      ":volume": self.volume
    };
    return (query, params.to_vec());
  }
}

fn get_regular_market_session_start_and_end(eastern_now: &DateTime<Tz>) -> (DateTime<Tz>, DateTime<Tz>) {
  let year = eastern_now.year();
  let month = eastern_now.month();
  let day = eastern_now.day();
  let regular_market_start = Eastern.with_ymd_and_hms(year, month, day, 9, 30, 0).unwrap(); // 9:30:00am
  let regular_market_end = Eastern.with_ymd_and_hms(year, month, day, 15, 59, 59).unwrap(); // 3:59:59pm
  return (regular_market_start, regular_market_end);
}

async fn align_to_top_of_second() {
  let now = Utc::now();
  let difference = 1000 - (now.timestamp_millis() % 1000);
  tokio::time::sleep(tokio::time::Duration::from_millis(difference as u64)).await;
}

fn main() {
  // logger
  simple_logger::SimpleLogger::new().env().init().unwrap();
  // runtime
  let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  // run
  rt.block_on(async {
    // config
    let provider_name = "yahoo_finance";
    let provider: Provider = provider_name.parse().unwrap();
    let symbol = "SPY";
    let resolution = "1";
    // open database
    let database = database::Database::new("./database.db");
    // init database tables
    database.migrate("./schema/");
    // loop
    loop {
      // check time
      let now = Utc::now();
      let eastern_now = now.with_timezone(&Eastern);
      let (regular_market_start, regular_market_end) = get_regular_market_session_start_and_end(&eastern_now);
      // before market start
      if now < regular_market_start {
        log::warn!("now < regular_market_start");
        align_to_top_of_second().await;
        continue;
      }
      // after market end
      if now > regular_market_end {
        log::warn!("now >= regular_market_end");
        align_to_top_of_second().await;
        continue;
      }
      // weekend
      let weekday = eastern_now.weekday();
      let is_weekend = weekday == Weekday::Sat || weekday == Weekday::Sun;
      if is_weekend == true {
        log::warn!("is_weekend == true");
        align_to_top_of_second().await;
        continue;
      }
      // holiday
      let is_holiday = false; // TODO
      if is_holiday == true {
        log::warn!("is_holiday == true");
        align_to_top_of_second().await;
        continue;
      }
      // get candle
      // TODO: support scraping historical candles?
      let result = match provider {
        Provider::Finnhub => providers::finnhub::get_candles(symbol, resolution, regular_market_start, regular_market_end).await,
        Provider::YahooFinance => providers::yahoo_finance::get_candles(symbol, resolution, regular_market_start, regular_market_end).await,
        Provider::Polygon => providers::polygon::get_candles(symbol, resolution, regular_market_start, regular_market_end).await,
      };
      if result.is_err() {
        log::error!("failed to get candles: {:?}", result);
        align_to_top_of_second().await;
        continue;
      }
      let candles = result.unwrap();
      if candles.len() == 0 {
        log::warn!("no candles");
        align_to_top_of_second().await;
        continue;
      }
      let most_recent_candle = &candles[candles.len() - 1];
      // log
      log::info!("{:?}", most_recent_candle);
      // insert most recent candle into database
      let result = database.insert(most_recent_candle);
      if result.is_err() {
        log::error!("failed to insert into database: {:?}", result);
        align_to_top_of_second().await;
        continue;
      }
      // TODO: store more than just the most recent candle?
      // sleep
      align_to_top_of_second().await;
    }
  });
}
