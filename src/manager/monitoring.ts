/**
 * Production Monitoring Module for HyperArb
 * Provides comprehensive logging, metrics, and alerting
 */

import { EventEmitter } from 'events';
import { appendFileSync, existsSync, mkdirSync } from 'fs';
import { join } from 'path';

export enum LogLevel {
    DEBUG = 0,
    INFO = 1,
    WARN = 2,
    ERROR = 3,
    CRITICAL = 4
}

export interface MonitoringConfig {
    logLevel: LogLevel;
    logToFile: boolean;
    logDir: string;
    metricsEnabled: boolean;
    alertWebhookUrl?: string;
    heartbeatInterval: number; // in ms
}

export interface TradeMetrics {
    timestamp: number;
    type: 'statistical' | 'double-leg';
    dex: string;
    pool: string;
    baseToken: string;
    quoteToken: string;
    tradeSize: number;
    tradeSizeUsd: number;
    expectedProfit: number;
    actualProfit?: number;
    gasUsed?: number;
    gasCostUsd?: number;
    success: boolean;
    errorMessage?: string;
}

export interface InventoryMetrics {
    timestamp: number;
    vault: string;
    baseToken: string;
    quoteToken: string;
    baseBalance: number;
    quoteBalance: number;
    baseValueUsd: number;
    quoteValueUsd: number;
    totalValueUsd: number;
    basePercentage: number;
    quotePercentage: number;
    isBalanced: boolean;
}

export interface SystemMetrics {
    timestamp: number;
    blockNumber: number;
    gasPrice: number;
    poolsChecked: number;
    opportunitiesFound: number;
    tradesExecuted: number;
    successRate: number;
    totalProfitUsd: number;
    uptime: number;
    memoryUsage: number;
    cpuUsage?: number;
}

class ProductionMonitor extends EventEmitter {
    private config: MonitoringConfig;
    private startTime: number;
    private tradeHistory: TradeMetrics[] = [];
    private systemMetrics: SystemMetrics[] = [];
    private lastHeartbeat: number = Date.now();
    private heartbeatTimer?: NodeJS.Timeout;
    
    constructor(config: MonitoringConfig) {
        super();
        this.config = config;
        this.startTime = Date.now();
        
        // Ensure log directory exists
        if (config.logToFile && !existsSync(config.logDir)) {
            mkdirSync(config.logDir, { recursive: true });
        }
        
        // Start heartbeat monitoring
        if (config.heartbeatInterval > 0) {
            this.startHeartbeat();
        }
    }
    
    // Structured logging with levels
    public log(level: LogLevel, component: string, message: string, data?: any): void {
        if (level < this.config.logLevel) return;
        
        const timestamp = new Date().toISOString();
        const levelStr = LogLevel[level];
        
        const logEntry = {
            timestamp,
            level: levelStr,
            component,
            message,
            data,
            processUptime: this.getUptime()
        };
        
        // Console output with color coding
        const color = this.getLogColor(level);
        console.log(
            `${color}[${timestamp}] [${levelStr}] [${component}] ${message}${this.resetColor()}`,
            data ? JSON.stringify(data, null, 2) : ''
        );
        
        // File logging
        if (this.config.logToFile) {
            this.writeToFile(logEntry);
        }
        
        // Alert on critical errors
        if (level >= LogLevel.ERROR) {
            this.sendAlert(levelStr, component, message, data);
        }
        
        // Emit for external handlers
        this.emit('log', logEntry);
    }
    
    // Trade execution monitoring
    public recordTrade(metrics: TradeMetrics): void {
        this.tradeHistory.push(metrics);
        
        // Keep only last 1000 trades in memory
        if (this.tradeHistory.length > 1000) {
            this.tradeHistory.shift();
        }
        
        this.log(
            metrics.success ? LogLevel.INFO : LogLevel.ERROR,
            'TRADE',
            `${metrics.type} trade ${metrics.success ? 'completed' : 'failed'}`,
            {
                dex: metrics.dex,
                pool: metrics.pool,
                tradeSizeUsd: metrics.tradeSizeUsd,
                profit: metrics.actualProfit || metrics.expectedProfit,
                error: metrics.errorMessage
            }
        );
        
        // Update success rate
        this.updateSuccessRate();
        
        this.emit('trade', metrics);
    }
    
    // Inventory monitoring
    public recordInventory(metrics: InventoryMetrics): void {
        const logLevel = metrics.isBalanced ? LogLevel.DEBUG : LogLevel.WARN;
        
        this.log(
            logLevel,
            'INVENTORY',
            `${metrics.vault} inventory ${metrics.isBalanced ? 'balanced' : 'IMBALANCED'}`,
            {
                basePercentage: metrics.basePercentage.toFixed(2),
                quotePercentage: metrics.quotePercentage.toFixed(2),
                totalValueUsd: metrics.totalValueUsd.toFixed(2)
            }
        );
        
        this.emit('inventory', metrics);
    }
    
    // System metrics collection
    public recordSystemMetrics(metrics: Partial<SystemMetrics>): void {
        const fullMetrics: SystemMetrics = {
            timestamp: Date.now(),
            blockNumber: metrics.blockNumber || 0,
            gasPrice: metrics.gasPrice || 0,
            poolsChecked: metrics.poolsChecked || 0,
            opportunitiesFound: metrics.opportunitiesFound || 0,
            tradesExecuted: this.getTradesInLastHour(),
            successRate: this.getSuccessRate(),
            totalProfitUsd: this.getTotalProfit(),
            uptime: this.getUptime(),
            memoryUsage: process.memoryUsage().heapUsed / 1024 / 1024, // MB
            cpuUsage: metrics.cpuUsage
        };
        
        this.systemMetrics.push(fullMetrics);
        
        // Keep only last 24 hours of metrics
        const dayAgo = Date.now() - 24 * 60 * 60 * 1000;
        this.systemMetrics = this.systemMetrics.filter(m => m.timestamp > dayAgo);
        
        this.emit('metrics', fullMetrics);
    }
    
    // Health check
    public isHealthy(): boolean {
        const now = Date.now();
        const timeSinceHeartbeat = now - this.lastHeartbeat;
        
        // Consider unhealthy if no heartbeat for 5x the interval
        if (this.config.heartbeatInterval > 0 && 
            timeSinceHeartbeat > this.config.heartbeatInterval * 5) {
            return false;
        }
        
        // Check for recent successful trades
        const recentTrades = this.tradeHistory.filter(
            t => t.timestamp > now - 3600000 // Last hour
        );
        
        if (recentTrades.length > 0) {
            const successRate = recentTrades.filter(t => t.success).length / recentTrades.length;
            if (successRate < 0.5) return false; // Less than 50% success rate
        }
        
        return true;
    }
    
    // Heartbeat monitoring
    private startHeartbeat(): void {
        this.heartbeatTimer = setInterval(() => {
            this.lastHeartbeat = Date.now();
            this.emit('heartbeat', {
                timestamp: this.lastHeartbeat,
                healthy: this.isHealthy(),
                uptime: this.getUptime(),
                tradesExecuted: this.tradeHistory.length,
                successRate: this.getSuccessRate()
            });
        }, this.config.heartbeatInterval);
    }
    
    // Alert system
    private async sendAlert(level: string, component: string, message: string, data?: any): Promise<void> {
        if (!this.config.alertWebhookUrl) return;
        
        const alert = {
            level,
            component,
            message,
            data,
            timestamp: new Date().toISOString(),
            system: 'HyperArb',
            uptime: this.getUptime()
        };
        
        try {
            // Send to webhook (Discord, Slack, etc.)
            await fetch(this.config.alertWebhookUrl, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(alert)
            });
        } catch (error) {
            console.error('Failed to send alert:', error);
        }
        
        this.emit('alert', alert);
    }
    
    // Metrics calculations
    private getUptime(): number {
        return Math.floor((Date.now() - this.startTime) / 1000); // seconds
    }
    
    private getSuccessRate(): number {
        if (this.tradeHistory.length === 0) return 1;
        const successful = this.tradeHistory.filter(t => t.success).length;
        return successful / this.tradeHistory.length;
    }
    
    private getTotalProfit(): number {
        return this.tradeHistory.reduce((sum, t) => {
            return sum + (t.actualProfit || t.expectedProfit || 0);
        }, 0);
    }
    
    private getTradesInLastHour(): number {
        const hourAgo = Date.now() - 3600000;
        return this.tradeHistory.filter(t => t.timestamp > hourAgo).length;
    }
    
    private updateSuccessRate(): void {
        const rate = this.getSuccessRate();
        if (rate < 0.3) {
            this.log(
                LogLevel.CRITICAL,
                'MONITOR',
                `CRITICAL: Success rate dropped to ${(rate * 100).toFixed(1)}%`,
                { recentTrades: this.tradeHistory.slice(-10) }
            );
        }
    }
    
    // File logging
    private writeToFile(logEntry: any): void {
        try {
            const date = new Date().toISOString().split('T')[0];
            const filename = join(this.config.logDir, `hyperarb-${date}.log`);
            appendFileSync(filename, JSON.stringify(logEntry) + '\n');
        } catch (error) {
            console.error('Failed to write log to file:', error);
        }
    }
    
    // Color coding for console
    private getLogColor(level: LogLevel): string {
        switch (level) {
            case LogLevel.DEBUG: return '\x1b[36m'; // Cyan
            case LogLevel.INFO: return '\x1b[32m';  // Green
            case LogLevel.WARN: return '\x1b[33m';  // Yellow
            case LogLevel.ERROR: return '\x1b[31m'; // Red
            case LogLevel.CRITICAL: return '\x1b[35m'; // Magenta
            default: return '\x1b[0m';
        }
    }
    
    private resetColor(): string {
        return '\x1b[0m';
    }
    
    // Cleanup
    public shutdown(): void {
        if (this.heartbeatTimer) {
            clearInterval(this.heartbeatTimer);
        }
        
        this.log(LogLevel.INFO, 'MONITOR', 'Monitoring system shutting down', {
            totalTrades: this.tradeHistory.length,
            successRate: this.getSuccessRate(),
            totalProfit: this.getTotalProfit(),
            uptime: this.getUptime()
        });
        
        this.removeAllListeners();
    }
    
    // Export metrics for dashboards
    public getMetrics(): {
        trades: TradeMetrics[];
        system: SystemMetrics[];
        health: boolean;
        uptime: number;
        successRate: number;
        totalProfit: number;
    } {
        return {
            trades: this.tradeHistory.slice(-100), // Last 100 trades
            system: this.systemMetrics.slice(-100), // Last 100 metrics
            health: this.isHealthy(),
            uptime: this.getUptime(),
            successRate: this.getSuccessRate(),
            totalProfit: this.getTotalProfit()
        };
    }
}

// Singleton instance
let monitorInstance: ProductionMonitor | null = null;

export function initializeMonitoring(config: MonitoringConfig): ProductionMonitor {
    if (!monitorInstance) {
        monitorInstance = new ProductionMonitor(config);
    }
    return monitorInstance;
}

export function getMonitor(): ProductionMonitor {
    if (!monitorInstance) {
        throw new Error('Monitoring not initialized. Call initializeMonitoring first.');
    }
    return monitorInstance;
}

export default { initializeMonitoring, getMonitor, LogLevel };